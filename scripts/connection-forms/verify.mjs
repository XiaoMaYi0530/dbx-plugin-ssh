// Standalone SSH connection-form contract verifier.
//
// The monorepo version imports the DBX host's TypeScript condition evaluator.
// This copy intentionally keeps only the small, host-compatible evaluator and
// SSH assertions needed by this repository, so clean clones do not need ../host
// or ../shared. Replace this file with the future public form-contract package
// when that package is available; keep the scenarios below as the SSH
// regression contract.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const root = new URL("../../", import.meta.url);
const manifest = JSON.parse(readFileSync(new URL("manifest.json", root), "utf8"));
const provider = manifest.contributions.find((item) => item.type === "connection-provider");
assert(provider, "SSH manifest must define a connection provider");
const fields = provider.fields;
const byKey = Object.fromEntries(fields.map((field) => [field.key, field]));
const defaults = Object.fromEntries(fields.map((field) => [field.key, field.default]));
const locales = ["en", "zh-CN", "zh-TW", "es", "it", "ja", "pt-BR"];

assert.equal(new Set(fields.map((field) => field.key)).size, fields.length, "duplicate field keys");

function conditionMatches(condition, value) {
  if (!condition) return true;
  if (value === undefined || value === null || (typeof value === "string" && value.trim() === "")) return false;
  return condition.one_of.includes(String(value));
}

function isVisible(field, values, seen = new Set([field.key])) {
  const condition = field.visible_when;
  if (!condition || !conditionMatches(condition, values[condition.field])) return !condition;
  const target = byKey[condition.field];
  if (!target || seen.has(target.key)) return true;
  seen.add(target.key);
  return isVisible(target, values, seen);
}

function isRequired(field, values) {
  return Boolean(field.required)
    || Boolean(field.required_when && conditionMatches(field.required_when, values[field.required_when.field]));
}

for (const [index, field] of fields.entries()) {
  for (const condition of [field.visible_when, field.required_when].filter(Boolean)) {
    const target = byKey[condition.field];
    assert(target, `${field.key}: unknown condition field ${condition.field}`);
    assert(fields.indexOf(target) < index, `${field.key}: condition target must precede dependent field`);
    const values = target.type === "boolean" ? ["true", "false"] : target.options?.map((option) => option.value);
    if (values) assert(condition.one_of.every((value) => values.includes(value)), `${field.key}: invalid condition value`);
  }
  if (field.type === "select" && field.default !== undefined) {
    assert(field.options.some((option) => option.value === field.default), `${field.key}: invalid default`);
  }
  for (const locale of locales) {
    const localized = manifest.localizations[locale]?.contributions?.[provider.id]?.fields?.[field.key]
      ?? (locale === "en" ? field : undefined);
    assert(localized?.label?.trim(), `${locale}/${field.key}: missing label`);
    for (const option of field.options ?? []) {
      const label = Array.isArray(localized.options)
        ? localized.options.find((item) => item.value === option.value)?.label
        : localized.options?.[option.value];
      assert(label?.trim(), `${locale}/${field.key}/${option.value}: missing option label`);
    }
  }
}

const options = (key) => byKey[key].options.map((option) => option.value);
let scenarios = 0;
function state(overrides) {
  scenarios++;
  const values = { ...defaults, ...overrides };
  const visible = new Set(fields.filter((field) => isVisible(field, values)).map((field) => field.key));
  const required = new Set(fields.filter((field) => visible.has(field.key) && isRequired(field, values)).map((field) => field.key));
  return {
    visible(key, expected) { assert.equal(visible.has(key), expected, `${key} visibility: ${JSON.stringify(overrides)}`); },
    required(key, expected) { assert.equal(required.has(key), expected, `${key} required: ${JSON.stringify(overrides)}`); },
  };
}

for (const authentication of options("authentication")) {
  for (const sudo_source of options("sudo_source")) {
    for (const auth_flow_mode of options("auth_flow_mode")) {
      for (const read_only of [false, true]) {
        const current = state({ authentication, sudo_source, auth_flow_mode, read_only });
        const password = ["password", "private-key-password"].includes(authentication);
        const privateKey = ["private-key", "private-key-password"].includes(authentication);
        current.visible("password", password); current.required("password", password);
        current.visible("private_key_path", privateKey); current.required("private_key_path", privateKey);
        current.visible("private_key_passphrase", privateKey); current.required("private_key_passphrase", false);
        current.visible("agent_socket", authentication === "agent");
        current.visible("sudo_password", sudo_source === "custom");
        current.visible("sudo_profile", sudo_source === "global");
        current.visible("auth_flow_mode", sudo_source !== "global");
        current.visible("totp_secret", sudo_source !== "global" && auth_flow_mode !== "password_only");
        current.visible("totp_prompt_hint", sudo_source !== "global" && auth_flow_mode !== "password_only");
      }
    }
  }
}

assert.equal(byKey.sudo_whitelist.type, "textarea");
console.log(`PASS SSH connection form: ${scenarios} combinations; field ordering and seven-language labels/options`);
