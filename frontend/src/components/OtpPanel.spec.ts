// @vitest-environment happy-dom
// OtpPanel 组件测试：otp/list 渲染与 TOTP 取码、复制、发送到终端
// （ssh/terminal/in 通道的 sequenced-input 帧）、绑定管理（host.listConnections
// 与手输回退）、编辑保存参数（编辑不带 secret / 新建带 secret）、删除确认、
// 扫码导入预填。Dialog 内容 portal 到 body，按 FolderPickerDialog.spec 的
// 方式直接查询 document.body。
import { afterEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import OtpPanel from "./OtpPanel.vue";

// t 假实现：key 原样返回、参数拼在括号里，断言与 i18n 表解耦且可定位元素。
const t = (key: string, values?: Record<string, string | number>) =>
  values ? `${key}(${Object.values(values).join(",")})` : key;

const ENTRIES = [
  { id: "e1", otpType: "totp", issuer: "ACME", username: "dev", hasSecret: true, algorithm: "SHA1", digits: 6, period: 30, counter: null },
  { id: "e2", otpType: "hotp", issuer: "Legacy", username: "", hasSecret: true, algorithm: "SHA1", digits: 6, period: 30, counter: 4 },
];

interface OtpPanelHost {
  invoke: ReturnType<typeof vi.fn>;
  request: ReturnType<typeof vi.fn>;
  sendBinary: ReturnType<typeof vi.fn>;
  context?: Record<string, unknown>;
}

function installHost(overrides: {
  invoke?: (method: string, params?: Record<string, unknown>) => unknown;
  request?: (method: string) => unknown;
  listConnections?: unknown;
  withContext?: boolean;
} = {}) {
  const invoke = vi.fn(async (method: string, params?: Record<string, unknown>) => {
    if (overrides.invoke) return overrides.invoke(method, params);
    if (method === "otp/list") return { entries: ENTRIES, bindings: { "conn-a": "e1" } };
    if (method === "otp/generate") return { code: "246810", remainingSeconds: 18 };
    if (method === "ssh/sessions/list") return { sessions: [{ sessionId: "s-1", connectionId: "conn-a", workbenchId: "wb-1", connected: true }] };
    throw new Error(`unexpected method ${method}`);
  });
  const request = vi.fn(async (method: string) => {
    if (method === "host.listConnections") {
      if (overrides.listConnections === undefined) return { connections: [{ id: "conn-a", name: "Prod" }, { id: "conn-b", name: "Staging" }] };
      if (overrides.listConnections instanceof Error) throw overrides.listConnections;
      return overrides.listConnections;
    }
    return null;
  });
  const sendBinary = vi.fn(async () => undefined);
  const host: OtpPanelHost = { invoke, request, sendBinary };
  if (overrides.withContext !== false) host.context = { connectionId: "conn-a", workbenchId: "wb-1", plugin: { mode: "ssh" } };
  (window as unknown as { dbxPlugin: unknown }).dbxPlugin = host;
  return host;
}

function mountPanel() {
  return mount(OtpPanel, { props: { t } });
}

const q = <T extends HTMLElement>(selector: string) => document.body.querySelector<T>(selector);
const qa = <T extends HTMLElement>(selector: string) => [...document.body.querySelectorAll<T>(selector)];

function panelButton(wrapper: ReturnType<typeof mount>, title: string) {
  const button = wrapper.findAll("button").find((candidate) => candidate.attributes("title") === title);
  expect(button, `panel button ${title} exists`).toBeTruthy();
  return button!;
}

afterEach(() => {
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("OtpPanel", () => {
  it("loads otp/list, renders entries with type badges and fetches TOTP codes", async () => {
    const host = installHost();
    const wrapper = mountPanel();
    await flushPromises();
    const text = wrapper.text();
    expect(text).toContain("ACME");
    expect(text).toContain("Legacy");
    expect(text).toContain("otpPanel.totp");
    expect(text).toContain("otpPanel.hotp");
    expect(text).toContain("246810");
    // 倒计时数字与进度条随取码结果渲染。
    expect(wrapper.find(".otp-count").text()).toBe("18");
    expect(wrapper.find(".otp-count").attributes("title")).toBe("otpPanel.countdown");
    expect(wrapper.find(".otp-progress-fill").attributes("style")).toContain("60%");
    expect(host.invoke).toHaveBeenCalledWith("otp/generate", { entryId: "e1" });
    // HOTP 不自动取码，等用户点「生成」。
    expect(host.invoke).not.toHaveBeenCalledWith("otp/generate", { entryId: "e2" });
  });

  it("copies the current code through the clipboard bridge", async () => {
    installHost();
    // 沙箱 iframe 里 navigator.clipboard 常被拒：写路径最终落到 execCommand 兜底
    // （happy-dom 未实现 execCommand，用可写的 document 属性打桩）。
    const documentWithCommand = document as unknown as { execCommand?: (command: string) => boolean };
    const calls: string[] = [];
    documentWithCommand.execCommand = (command: string) => {
      calls.push(command);
      return true;
    };
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.copy").trigger("click");
    await flushPromises();
    expect(calls).toEqual(["copy"]);
    // 复制成功后面板给出已复制反馈（图标切换为对勾，title 同步）。
    expect(panelButton(wrapper, "otpPanel.copied")).toBeTruthy();
  });

  it("sends the code to the live session's terminal input channel without pressing enter", async () => {
    const host = installHost();
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.send").trigger("click");
    await flushPromises();
    expect(host.sendBinary).toHaveBeenCalledTimes(1);
    const [channel, payload] = host.sendBinary.mock.calls[0] as [string, Uint8Array];
    expect(channel).toBe("ssh/terminal/in/s-1");
    // 前 8 字节 BE 序号，其后是验证码字节；不带换行（不回车）。
    const bytes = Uint8Array.from(payload);
    expect(new DataView(bytes.buffer, bytes.byteOffset, 8).getBigUint64(0, false)).toBe(1n);
    expect(new TextDecoder().decode(bytes.subarray(8))).toBe("246810");
  });

  it("shows a readable hint when no live session can receive the code", async () => {
    // 唯一会话 connected:false 视为死会话：目标挑选返回空 → 展示可读提示。
    installHost({ invoke: (method) => {
      if (method === "otp/list") return { entries: ENTRIES, bindings: {} };
      if (method === "otp/generate") return { code: "246810", remainingSeconds: 18 };
      if (method === "ssh/sessions/list") return { sessions: [{ sessionId: "s-1", connectionId: "conn-a", workbenchId: "wb-1", connected: false }] };
      throw new Error(`unexpected method ${method}`);
    } });
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.send").trigger("click");
    await flushPromises();
    expect(wrapper.text()).toContain("otpPanel.send.noSession");
  });

  it("unbinds a bound connection from the bind section", async () => {
    const host = installHost();
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.bind").trigger("click");
    expect(wrapper.text()).toContain("Prod");
    await panelButton(wrapper, "otpPanel.unbind").trigger("click");
    await flushPromises();
    expect(host.invoke).toHaveBeenCalledWith("otp/unbind", { connectionId: "conn-a" });
  });

  it("falls back to a manual connection-id input when the host lacks listConnections", async () => {
    installHost({ listConnections: new Error("unknown method") });
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.bind").trigger("click");
    const input = wrapper.find(".otp-bind-manual");
    expect(input.exists()).toBe(true);
    expect(wrapper.text()).toContain("otpPanel.bindManualHint");
    await input.setValue("conn-manual");
    await input.trigger("keydown.enter");
    await flushPromises();
    expect(window.dbxPlugin.invoke).toHaveBeenCalledWith("otp/bind", { connectionId: "conn-manual", entryId: "e1" });
  });

  it("edits an entry keeping the stored secret when the field is left empty", async () => {
    const host = installHost();
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.edit").trigger("click");
    await flushPromises();
    const issuer = q<HTMLInputElement>(".otp-editor-field input")!;
    expect(issuer.value).toBe("ACME");
    const secret = q<HTMLInputElement>("input[type=password]")!;
    expect(secret.value).toBe("");
    issuer.value = "ACME2";
    issuer.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
    await q<HTMLFormElement>(".otp-editor")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    await flushPromises();
    const saveCall = host.invoke.mock.calls.find(([method]) => method === "otp/save") as unknown[] | undefined;
    expect(saveCall).toBeTruthy();
    expect(saveCall![1]).toEqual({ id: "e1", otpType: "totp", issuer: "ACME2", username: "dev", algorithm: "SHA1", digits: 6, period: 30 });
  });

  it("saves a new entry with the secret and blocks an empty issuer", async () => {
    const host = installHost();
    const wrapper = mountPanel();
    await flushPromises();
    const add = wrapper.find(".otp-add");
    expect(add.exists()).toBe(true);
    await add.trigger("click");
    await flushPromises();
    await q<HTMLFormElement>(".otp-editor")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    expect(document.body.textContent).toContain("otpPanel.error.issuer");
    expect(host.invoke.mock.calls.some(([method]) => method === "otp/save")).toBe(false);
  });

  it("deletes an entry after confirmation", async () => {
    const host = installHost({ invoke: (method, params) => {
      if (method === "otp/list") return { entries: ENTRIES, bindings: {} };
      if (method === "otp/generate") return { code: "246810", remainingSeconds: 18 };
      if (method === "otp/delete") return { deleted: true };
      if (method === "ssh/sessions/list") return { sessions: [] };
      void params;
      throw new Error(`unexpected method ${method}`);
    } });
    const wrapper = mountPanel();
    await flushPromises();
    await panelButton(wrapper, "otpPanel.delete").trigger("click");
    await flushPromises();
    expect(document.body.textContent).toContain("otpPanel.deleteConfirm.message(ACME)");
    const confirm = qa("button").find((button) => button.textContent?.includes("otpPanel.deleteConfirm.confirm"));
    expect(confirm).toBeTruthy();
    await confirm!.dispatchEvent(new MouseEvent("click", { bubbles: true, composed: true }));
    await flushPromises();
    expect(host.invoke).toHaveBeenCalledWith("otp/delete", { id: "e1" });
  });

  it("prefills the editor from an imported QR image", async () => {
    const host = installHost({ invoke: (method) => {
      if (method === "otp/list") return { entries: [], bindings: {} };
      if (method === "otp/import-qr") {
        return { otpType: "totp", issuer: "ACME", label: "ACME:dev@example.com", secretBase32: "GEZDGNBVGY3TQOJQ", algorithm: "SHA1", digits: 8, period: 60, counter: null };
      }
      if (method === "ssh/sessions/list") return { sessions: [] };
      throw new Error(`unexpected method ${method}`);
    } });
    const wrapper = mountPanel();
    await flushPromises();
    const input = wrapper.find<HTMLInputElement>("input[type=file]");
    const file = new File(["fake-png-bytes"], "qr.png", { type: "image/png" });
    Object.defineProperty(input.element, "files", { value: [file], configurable: true });
    await input.trigger("change");
    await flushPromises();
    expect(host.invoke).toHaveBeenCalledWith("otp/import-qr", { imageBase64: btoa("fake-png-bytes") });
    // 扫码预填后 showSecret 打开：密钥落在可切换的 textarea 里。
    const secret = document.body.querySelector<HTMLTextAreaElement>("textarea");
    expect(secret?.value).toBe("GEZDGNBVGY3TQOJQ");
  });

  it("generates an HOTP code on demand", async () => {
    const host = installHost({ invoke: (method, params) => {
      if (method === "otp/list") return { entries: ENTRIES, bindings: {} };
      if (method === "otp/generate") {
        void params;
        return { code: "112233", hotp: true };
      }
      if (method === "ssh/sessions/list") return { sessions: [] };
      throw new Error(`unexpected method ${method}`);
    } });
    const wrapper = mountPanel();
    await flushPromises();
    const generate = wrapper.findAll("button").find((button) => button.text().includes("otpPanel.generate"));
    expect(generate).toBeTruthy();
    await generate!.trigger("click");
    await flushPromises();
    expect(host.invoke).toHaveBeenCalledWith("otp/generate", { entryId: "e2" });
    expect(wrapper.text()).toContain("112233");
  });
});
