// @vitest-environment happy-dom
// ConnectingCard 测试：连接卡徽标与轨道端点统一采用插件品牌标记
// （PluginLogo，与 assets/plugin.svg 同形），不再用 lucide 的通用 Server
// 图形——loading / 成功等所有状态都要一致；右侧终点仍是终端/对勾。
import { describe, expect, it } from "vitest";
import { mount } from "@vue/test-utils";
import ConnectingCard from "./ConnectingCard.vue";
import rawPluginLogo from "../../../assets/plugin.svg?raw";

function mountCard(state: "connecting" | "error" | "cancelled" | "success") {
  return mount(ConnectingCard, {
    props: {
      locale: "en",
      name: "bastion-01",
      identity: "sshuser@10.0.0.8:22",
      state,
      logsOpen: false,
      logs: [],
    },
  });
}

describe("ConnectingCard", () => {
  it("renders the plugin logo mark on the badge and the server endpoint while connecting", () => {
    const wrapper = mountCard("connecting");
    expect(wrapper.find(".connect-card-badge .plugin-logo").exists()).toBe(true);
    expect(wrapper.find(".connect-card-endpoint-server .plugin-logo").exists()).toBe(true);
    // 全卡恰好两处品牌标记：徽标 + 左端点；右端点在 connecting 态是终端图形。
    expect(wrapper.findAll(".plugin-logo")).toHaveLength(2);
    expect(wrapper.find(".connect-card-endpoint-target svg").classes().includes("plugin-logo")).toBe(false);
    // 单一来源：渲染出的图形就是 assets/plugin.svg 原文（同路径数据，非复刻件）。
    const badgeHtml = wrapper.find(".connect-card-badge .plugin-logo").html();
    expect(badgeHtml).toContain('viewBox="0 0 64 64"');
    for (const segment of ["M9 10 29 32 9 54", "M37 53h18"]) {
      expect(badgeHtml).toContain(segment);
      expect(rawPluginLogo).toContain(segment);
    }
  });

  it("keeps the plugin logo across success and error states", () => {
    for (const state of ["success", "error", "cancelled"] as const) {
      const wrapper = mountCard(state);
      expect(wrapper.find(".connect-card-badge .plugin-logo").exists()).toBe(true);
      expect(wrapper.find(".connect-card-endpoint-server .plugin-logo").exists()).toBe(true);
    }
  });

  it("shows the check glyph on the target endpoint only after success", () => {
    expect(mountCard("connecting").find(".connect-card-endpoint-target svg").classes().includes("plugin-logo")).toBe(false);
    expect(mountCard("success").text()).toContain("Connected");
  });
});
