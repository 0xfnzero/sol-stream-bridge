import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "./App";
import { limitsFromForm, type Snapshot } from "./api";

const adminToken = "test-only-administrator-token-32bytes";
const loginPassword = "test-password-twelve-characters";
const loginResponse = {
  token: adminToken,
  username: "sb_test01",
  must_change_password: false,
  expires_in_seconds: 28800,
};
const secret = "a".repeat(64);
const initial: Snapshot = {
  revision: "0",
  require_api_key: true,
  allowed_sources: ["127.0.0.1"],
  users: [],
  subscriptions: [],
  uptime_seconds: 20,
  metrics: {
    connections: 0,
    subscriptions: 0,
    upstream_connections: 0,
    upstream_streams: 0,
    filters: 0,
    accounts: 0,
    rejected: 0,
    slow_clients: 0,
    upstream_failures: 0,
    updates: 0,
    delivered: 0,
  },
  service: {
    version: "0.1.1",
    grpc_listen: "127.0.0.1:10000",
    commitment: "processed",
    client_tls: false,
    max_connections: 64,
    max_subscriptions: 64,
    max_filters: 64,
    max_accounts: 512,
    allow_unfiltered: false,
    max_subscriptions_per_key: 2,
    max_filters_per_key: 16,
    max_accounts_per_key: 256,
    allow_accounts: false,
  },
};
const fetchMock = vi.fn<typeof fetch>();
let snapshot: Snapshot;
const reply = (data: unknown, status = 200) =>
  Promise.resolve(
    new Response(JSON.stringify(data), {
      status,
      headers: { "Content-Type": "application/json" },
    }),
  );
beforeEach(() => {
  snapshot = structuredClone(initial);
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockImplementation((path) =>
    reply(path === "/api/auth/login" ? loginResponse : snapshot),
  );
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.setAttribute("open", "");
    },
  });
  Object.defineProperty(HTMLDialogElement.prototype, "close", {
    configurable: true,
    value: function (this: HTMLDialogElement) {
      this.removeAttribute("open");
    },
  });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});
async function login() {
  const user = userEvent.setup();
  render(<App />);
  await user.type(screen.getByLabelText("账号"), "sb_test01");
  await user.type(
    screen.getByLabelText("密码", { exact: true }),
    loginPassword,
  );
  await user.click(screen.getByRole("button", { name: "进入控制台" }));
  await screen.findByRole("heading", { name: "运行概览" });
  return user;
}

describe("operator session and live changes", () => {
  it("rejects invalid login and does not persist credentials", async () => {
    fetchMock.mockImplementation(() => reply({ error: "invalid" }, 401));
    const user = userEvent.setup();
    render(<App />);
    await user.type(screen.getByLabelText("账号"), "sb_test01");
    await user.type(
      screen.getByLabelText("密码", { exact: true }),
      loginPassword,
    );
    await user.click(screen.getByRole("button", { name: "进入控制台" }));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "账号或密码错误，请检查后重试。",
    );
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
    expect(fetchMock.mock.calls[0][1]).toMatchObject({
      credentials: "omit",
      cache: "no-store",
      headers: { "Content-Type": "application/json" },
      method: "POST",
      body: JSON.stringify({ username: "sb_test01", password: loginPassword }),
    });
  });
  it("creates a real request with quota settings and clears the one-time secret on logout", async () => {
    const user = await login();
    await user.click(screen.getAllByRole("button", { name: "添加用户" })[0]);
    await user.type(screen.getByLabelText("用户名"), "bot-01");
    await user.click(screen.getByRole("checkbox", { name: /允许账户更新/ }));
    fetchMock.mockImplementation((path, options) => {
      if (path === "/api/users" && options?.method === "POST") {
        const input = JSON.parse(String(options.body));
        expect(input).toEqual({
          revision: "0",
          name: "bot-01",
          limits: {
            max_subscriptions: 2,
            max_filters: 16,
            max_accounts: 256,
            allow_accounts: true,
          },
        });
        snapshot.revision = "1";
        snapshot.users = [
          {
            name: "bot-01",
            enabled: true,
            has_key: true,
            limits: input.limits,
            effective: input.limits,
            usage: { subscriptions: 0, pending: 0, filters: 0, accounts: 0 },
          },
        ];
        return reply({ revision: "1", api_key: secret });
      }
      return reply(snapshot);
    });
    await user.click(screen.getByRole("button", { name: "创建用户" }));
    const key = await screen.findByLabelText("API Key");
    expect(key).toHaveProperty("value", secret);
    expect(localStorage.length).toBe(0);
    expect(sessionStorage.length).toBe(0);
    await user.click(screen.getByRole("button", { name: "我已保存" }));
    expect(screen.queryByDisplayValue(secret)).toBeNull();
    await user.click(screen.getByRole("button", { name: "退出登录" }));
    expect(screen.getByLabelText("密码", { exact: true })).toHaveProperty(
      "value",
      "",
    );
    expect(screen.queryByText("bot-01")).toBeNull();
  });
  it("surfaces stale revisions without silently retrying a write", async () => {
    const user = await login();
    await user.click(screen.getAllByRole("button", { name: "添加用户" })[0]);
    await user.type(screen.getByLabelText("用户名"), "bot-02");
    fetchMock.mockImplementation((_path, options) =>
      options?.method === "POST"
        ? reply({ error: "Settings changed. Refresh and try again." }, 409)
        : reply({ ...snapshot, revision: "1" }),
    );
    await user.click(screen.getByRole("button", { name: "创建用户" }));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "Settings changed. Refresh and try again.",
    );
    expect(
      fetchMock.mock.calls.filter(
        ([path, o]) => path === "/api/users" && o?.method === "POST",
      ),
    ).toHaveLength(1);
  });
  it("requires confirmation before updating access and resets the saved draft", async () => {
    const user = await login();
    await user.click(screen.getByRole("button", { name: "访问策略" }));
    await user.click(
      screen.getByRole("checkbox", { name: "要求用户 API Key" }),
    );
    await user.click(screen.getByRole("button", { name: "保存访问策略" }));
    const dialog = screen.getByRole("dialog");
    expect(
      within(dialog).getByText(/切换验证方式会断开全部现有订阅/),
    ).toBeTruthy();
    expect(fetchMock.mock.calls.some(([, o]) => o?.method === "PUT")).toBe(
      false,
    );
    fetchMock.mockImplementation((path, options) => {
      if (path === "/api/access" && options?.method === "PUT") {
        expect(JSON.parse(String(options.body))).toEqual({
          revision: "0",
          require_api_key: false,
          allowed_sources: ["127.0.0.1"],
        });
        snapshot = { ...snapshot, revision: "1", require_api_key: false };
        return reply({ revision: "1" });
      }
      return reply(snapshot);
    });
    await user.click(within(dialog).getByRole("button", { name: "确认操作" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    await waitFor(() =>
      expect(
        screen.getByRole("button", { name: "保存访问策略" }),
      ).toHaveProperty("disabled", true),
    );
    expect(
      screen.queryByText("服务配置已更新。请先重新载入，再提交修改。"),
    ).toBeNull();
  });
});

it("rejects fractional, zero and excessive quota input", () => {
  const form = document.createElement("form");
  form.innerHTML =
    '<input name="max_subscriptions" value="1"><input name="max_filters" value="2"><input name="max_accounts" value="3">';
  expect(limitsFromForm(form).max_filters).toBe(2);
  const input = form.querySelector<HTMLInputElement>("[name=max_filters]")!;
  for (const invalid of ["0", "1.5", "4097", "-1", "Infinity"]) {
    input.value = invalid;
    expect(() => limitsFromForm(form)).toThrow();
  }
});

it("shows and hides the entered login password without exposing initial credentials", async () => {
  const user = userEvent.setup();
  render(<App />);
  const input = screen.getByLabelText("密码", { exact: true });
  await user.type(input, loginPassword);
  expect(input).toHaveProperty("type", "password");
  await user.click(screen.getByRole("button", { name: "显示密码" }));
  expect(input).toHaveProperty("type", "text");
  await user.click(screen.getByRole("button", { name: "隐藏密码" }));
  expect(input).toHaveProperty("type", "password");
  expect(fetchMock).not.toHaveBeenCalled();
});

it("requires a first-login password change before fetching operator data", async () => {
  fetchMock.mockImplementation((path) =>
    reply(
      path === "/api/auth/login"
        ? { ...loginResponse, must_change_password: true }
        : { ok: true },
    ),
  );
  const user = userEvent.setup();
  render(<App />);
  await user.type(screen.getByLabelText("账号"), "sb_test01");
  await user.type(
    screen.getByLabelText("密码", { exact: true }),
    loginPassword,
  );
  await user.click(screen.getByRole("button", { name: "进入控制台" }));
  await screen.findByRole("heading", { name: "设置你的新密码" });
  expect(fetchMock.mock.calls.some(([path]) => path === "/api/state")).toBe(
    false,
  );
  await user.type(
    screen.getByLabelText("当前密码", { exact: true }),
    loginPassword,
  );
  await user.type(
    screen.getByLabelText("新密码", { exact: true }),
    "new-password-for-test",
  );
  await user.type(
    screen.getByLabelText("确认新密码", { exact: true }),
    "different-password-for-test",
  );
  await user.click(screen.getByRole("button", { name: "保存新密码" }));
  expect(await screen.findByRole("alert")).toHaveProperty(
    "textContent",
    "两次输入的新密码不一致。",
  );
  await user.clear(screen.getByLabelText("确认新密码", { exact: true }));
  await user.type(
    screen.getByLabelText("确认新密码", { exact: true }),
    "new-password-for-test",
  );
  await user.click(screen.getByRole("button", { name: "保存新密码" }));
  await screen.findByRole("heading", { name: "连接你的转发服务" });
  expect(screen.getByRole("status").textContent).toContain("密码已更新");
  const mutation = fetchMock.mock.calls.find(
    ([path]) => path === "/api/auth/password",
  );
  expect(JSON.parse(String(mutation?.[1]?.body))).toEqual({
    current_password: loginPassword,
    new_password: "new-password-for-test",
  });
  expect(screen.getByLabelText("密码", { exact: true })).toHaveProperty(
    "value",
    "",
  );
});

it("allows an established operator to change their password", async () => {
  const user = await login();
  await user.click(screen.getByRole("button", { name: "修改密码" }));
  await user.type(
    screen.getByLabelText("当前密码", { exact: true }),
    loginPassword,
  );
  await user.type(
    screen.getByLabelText("新密码", { exact: true }),
    "new-password-for-test",
  );
  await user.type(
    screen.getByLabelText("确认新密码", { exact: true }),
    "new-password-for-test",
  );
  await user.click(screen.getByRole("button", { name: "保存新密码" }));
  expect(await screen.findByRole("status")).toHaveProperty(
    "textContent",
    "密码已更新，请使用新密码重新登录。其他登录会话已失效。",
  );
});
