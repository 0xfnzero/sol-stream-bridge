import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import {
  ApiError,
  limitsFromForm,
  request,
  type Effective,
  type Snapshot,
  type User,
} from "./api";

type Page = "overview" | "users" | "subscriptions" | "access";
const pages: { id: Page; title: string; icon: string }[] = [
  { id: "overview", title: "运行概览", icon: "grid" },
  { id: "users", title: "用户与密钥", icon: "users" },
  { id: "subscriptions", title: "活动订阅", icon: "activity" },
  { id: "access", title: "访问策略", icon: "shield" },
];
const iconPaths: Record<string, ReactNode> = {
  grid: (
    <>
      <rect x="3" y="3" width="7" height="7" rx="1.5" />
      <rect x="14" y="3" width="7" height="7" rx="1.5" />
      <rect x="3" y="14" width="7" height="7" rx="1.5" />
      <rect x="14" y="14" width="7" height="7" rx="1.5" />
    </>
  ),
  users: (
    <>
      <circle cx="9" cy="8" r="3" />
      <path d="M3 21v-3a6 6 0 0 1 12 0v3M16 5a3 3 0 0 1 0 6m2 4a5 5 0 0 1 3 5" />
    </>
  ),
  activity: <path d="M2 12h5l3-8 4 16 3-8h5" />,
  shield: (
    <>
      <path d="M12 3 3 7v5c0 5 9 9 9 9s9-4 9-9V7z" />
      <path d="m8 12 3 3 5-5" />
    </>
  ),
  arrow: <path d="M5 12h14m-5-5 5 5-5 5" />,
  plus: <path d="M12 4v16M4 12h16" />,
  key: (
    <>
      <circle cx="8" cy="9" r="5" />
      <path d="m12 13 8 8m-5-5 3-3m0 6 3-3" />
    </>
  ),
  logout: (
    <>
      <path d="M9 4H4v16h5m5-4 4-4-4-4m-5 4h10" />
    </>
  ),
  search: (
    <>
      <circle cx="10" cy="10" r="6" />
      <path d="m15 15 6 6" />
    </>
  ),
  refresh: (
    <>
      <path d="M20 7v5h-5M4 17v-5h5" />
      <path d="M6 7a7 7 0 0 1 12-1l2 3M4 15l2 3a7 7 0 0 0 12-1" />
    </>
  ),
  close: <path d="m6 6 12 12M6 18 18 6" />,
  check: <path d="m5 12 4 4L19 6" />,
  link: (
    <>
      <path d="M9 15 15 9m-7 7-1 1a4 4 0 0 1-6-6l5-5a4 4 0 0 1 6 0m0 12a4 4 0 0 0 6 0l5-5a4 4 0 0 0-6-6l-1 1" />
    </>
  ),
};
function Icon({ name, size = 18 }: { name: string; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {iconPaths[name] ?? iconPaths.activity}
    </svg>
  );
}
function Brand() {
  return (
    <div className="brand">
      <img src="/bridge.svg" alt="" />
      <div>
        <strong>Sol Stream Bridge</strong>
        <span>OPERATOR CONSOLE</span>
      </div>
    </div>
  );
}
const number = (n: number) => n.toLocaleString("zh-CN");
const duration = (seconds: number) =>
  seconds >= 86400
    ? `${Math.floor(seconds / 86400)} 天 ${Math.floor((seconds % 86400) / 3600)} 小时`
    : seconds >= 3600
      ? `${Math.floor(seconds / 3600)} 小时 ${Math.floor((seconds % 3600) / 60)} 分`
      : `${Math.floor(seconds / 60)} 分 ${Math.floor(seconds % 60)} 秒`;
const message = (e: unknown) =>
  e instanceof Error ? e.message : "操作失败，请重试。";

export default function App() {
  const [session, setSession] = useState<{
    token: string;
    initial: Snapshot;
  }>();
  return session ? (
    <Console
      key={session.token}
      token={session.token}
      initial={session.initial}
      logout={() => setSession(undefined)}
    />
  ) : (
    <Login onLogin={(token, initial) => setSession({ token, initial })} />
  );
}
function Login({
  onLogin,
}: {
  onLogin: (token: string, data: Snapshot) => void;
}) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setBusy(true);
    setError("");
    const token = String(
      new FormData(e.currentTarget).get("token") ?? "",
    ).trim();
    try {
      const data = await request<Snapshot>(token, "/api/state");
      onLogin(token, data);
    } catch (e) {
      setError(
        e instanceof ApiError && e.status === 401
          ? "管理员令牌无效，请检查后重试。"
          : "暂时无法连接管理服务，请确认服务已启动。",
      );
    } finally {
      setBusy(false);
    }
  }
  return (
    <main className="login-page">
      <div className="login-brand">
        <Brand />
      </div>
      <section className="login-card">
        <span className="eyebrow">YOUR STREAMS. YOUR CONTROL.</span>
        <h1>连接你的转发服务</h1>
        <p>
          管理订阅用户、分配访问额度，
          <br />
          在一个地方掌握服务运行状态。
        </p>
        <form onSubmit={submit}>
          <label htmlFor="token">管理员令牌</label>
          <input
            id="token"
            name="token"
            type="password"
            placeholder="输入 DASHBOARD_TOKEN"
            autoComplete="off"
            required
            minLength={32}
            maxLength={256}
            autoFocus
          />
          <small>令牌仅保留在当前页面，刷新或退出后需要重新登录。</small>
          {error && (
            <div className="alert error" role="alert">
              {error}
            </div>
          )}
          <button className="primary full" disabled={busy}>
            {busy ? "正在连接…" : "进入控制台"}
            <Icon name="arrow" />
          </button>
        </form>
        <div className="login-note">
          <Icon name="shield" />
          此入口仅供服务管理员使用
        </div>
      </section>
      <footer>
        Solana data, shared with control.<span>Sol Stream Bridge</span>
      </footer>
    </main>
  );
}

type ModalState =
  | { type: "user"; user?: User; revision: string }
  | {
      type: "confirm";
      title: string;
      description: string;
      action: () => Promise<void>;
      danger?: boolean;
    }
  | { type: "secret"; key: string };
interface Sample {
  at: number;
  inbound: number;
  delivered: number;
}
function Console({
  token,
  initial,
  logout,
}: {
  token: string;
  initial: Snapshot;
  logout: () => void;
}) {
  const [data, setData] = useState(initial);
  const [page, setPage] = useState<Page>("overview");
  const [error, setError] = useState("");
  const [online, setOnline] = useState(true);
  const [lastSync, setLastSync] = useState(new Date());
  const [modal, setModal] = useState<ModalState>();
  const [busy, setBusy] = useState(false);
  const [search, setSearch] = useState("");
  const [samples, setSamples] = useState<Sample[]>([]);
  const previous = useRef({ at: Date.now(), metrics: initial.metrics });
  const alive = useRef(true);
  const refreshSequence = useRef(0);
  const [accessGeneration, setAccessGeneration] = useState(0);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      const sequence = ++refreshSequence.current;
      try {
        const next = await request<Snapshot>(
          token,
          "/api/state",
          "GET",
          undefined,
          signal,
        );
        if (
          !alive.current ||
          signal?.aborted ||
          sequence !== refreshSequence.current
        )
          return;
        const at = Date.now(),
          elapsed = (at - previous.current.at) / 1000;
        if (elapsed > 0.25)
          setSamples((old) =>
            [
              ...old,
              {
                at,
                inbound:
                  Math.max(
                    0,
                    next.metrics.updates - previous.current.metrics.updates,
                  ) / elapsed,
                delivered:
                  Math.max(
                    0,
                    next.metrics.delivered - previous.current.metrics.delivered,
                  ) / elapsed,
              },
            ].slice(-30),
          );
        previous.current = { at, metrics: next.metrics };
        setData(next);
        setOnline(true);
        setLastSync(new Date());
      } catch (e) {
        if (
          !alive.current ||
          signal?.aborted ||
          sequence !== refreshSequence.current
        )
          return;
        if (e instanceof ApiError && e.status === 401) logout();
        else setOnline(false);
      }
    },
    [token, logout],
  );
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      await refreshRef.current(controller.signal);
      if (!controller.signal.aborted) timer = setTimeout(poll, 2000);
    };
    timer = setTimeout(poll, 2000);
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, []);
  async function mutate(path: string, method: string, payload?: unknown) {
    setBusy(true);
    setError("");
    try {
      const result = await request<{ api_key?: string | null }>(
        token,
        path,
        method,
        payload,
      );
      if (!alive.current) return;
      setModal(
        result.api_key ? { type: "secret", key: result.api_key } : undefined,
      );
      await refresh();
      if (path === "/api/access" && alive.current)
        setAccessGeneration((n) => n + 1);
    } catch (e) {
      if (!alive.current) return;
      if (e instanceof ApiError && e.status === 401) logout();
      else {
        setError(message(e));
        if (e instanceof ApiError && e.status === 409) await refresh();
      }
    } finally {
      if (alive.current) setBusy(false);
    }
  }
  const confirm = (
    title: string,
    description: string,
    action: () => Promise<void>,
    danger = false,
  ) => {
    setError("");
    setModal({ type: "confirm", title, description, action, danger });
  };
  const add = () => {
    setError("");
    setModal({ type: "user", revision: data.revision });
  };
  const userPath = (name: string) => `/api/users/${encodeURIComponent(name)}`;
  const status = !online
    ? "连接中断"
    : data.metrics.upstream_streams
      ? "正在转发"
      : data.metrics.subscriptions
        ? "等待上游"
        : "等待订阅";
  const latest = samples.at(-1);
  return (
    <div className="layout">
      <aside className="sidebar">
        <Brand />
        <div className="sidebar-label">工作空间</div>
        <nav>
          {pages.map((item) => (
            <button
              key={item.id}
              className={page === item.id ? "active" : ""}
              aria-current={page === item.id ? "page" : undefined}
              onClick={() => {
                setPage(item.id);
                setError("");
              }}
            >
              <Icon name={item.icon} />
              {item.title}
              {item.id === "users" && (
                <span className="nav-count">{data.users.length}</span>
              )}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="node-card">
            <span className={`dot ${online ? "" : "warn"}`} />
            <div>
              <strong>共享接收端</strong>
              <code>{data.service.grpc_listen}</code>
            </div>
          </div>
          <a
            href="https://github.com/0xfnzero/sol-stream-bridge/blob/main/docs/DASHBOARD_CN.md"
            target="_blank"
            rel="noreferrer"
          >
            使用文档 <Icon name="arrow" size={14} />
          </a>
          <div className="sidebar-version">
            v{data.service.version}
          </div>
        </div>
      </aside>
      <div className="workspace">
        <header className="topbar">
          <div className="breadcrumbs">
            控制台 <span>/</span> {pages.find((p) => p.id === page)?.title}
          </div>
          <div className="topbar-right">
            <span className="sync">
              <span className={`dot ${online ? "" : "warn"}`} />
              {online ? "每 2 秒刷新" : "服务暂不可达"}
            </span>
            <button
              className="icon-button"
              title="刷新数据"
              aria-label="刷新数据"
              onClick={() => void refresh()}
            >
              <Icon name="refresh" />
            </button>
            <span className="divider" />
            <span className="avatar">A</span>
            <span className="admin-label">管理员</span>
            <button
              className="icon-button"
              title="退出登录"
              aria-label="退出登录"
              onClick={logout}
            >
              <Icon name="logout" />
            </button>
          </div>
        </header>
        <main className="content">
          <div className="page-heading">
            <div>
              <div className="eyebrow">STREAM OPERATIONS</div>
              <h1>{pages.find((p) => p.id === page)?.title}</h1>
              <p>
                {
                  {
                    overview: "一条上游连接，让每个用户收到自己需要的数据。",
                    users: "分配用户密钥、订阅额度和账户更新权限。",
                    subscriptions: "查看正在接收数据的用户，按需断开指定订阅。",
                    access: "设置谁可以连接，以及连接时需要怎样的验证。",
                  }[page]
                }
              </p>
            </div>
            {(page === "overview" || page === "users") && (
              <button className="primary" onClick={add}>
                <Icon name="plus" />
                添加用户
              </button>
            )}
          </div>
          {!online && (
            <div role="alert" className="alert error">
              无法刷新服务状态，以下是最后一次成功获取的数据。连接恢复后会自动更新。
            </div>
          )}
          {error && !modal && (
            <div role="alert" className="alert error">
              {error}
            </div>
          )}
          {!data.require_api_key && (
            <div className="alert warning">
              <Icon name="shield" />
              <div>
                API Key 验证未开启。白名单内的 IP 可直接订阅，额度按来源 IP
                计算。
              </div>
              <button onClick={() => setPage("access")}>
                管理策略 <Icon name="arrow" size={14} />
              </button>
            </div>
          )}
          {page === "overview" && (
            <>
              <section className="metrics-grid">
                <Metric
                  title="上游订阅"
                  value={`${data.metrics.upstream_streams} / 1`}
                  detail={status}
                  icon="link"
                  positive={online && !!data.metrics.upstream_streams}
                />
                <Metric
                  title="活动订阅"
                  value={number(data.metrics.subscriptions)}
                  detail={`上限 ${number(data.service.max_subscriptions)} 条`}
                  icon="activity"
                />
                <Metric
                  title="已配置用户"
                  value={number(data.users.length)}
                  detail={`${data.users.filter((u) => u.enabled).length} 个已启用`}
                  icon="users"
                />
                <Metric
                  title="分发速率"
                  value={
                    latest ? `${number(Math.round(latest.delivered))}` : "—"
                  }
                  suffix="条/秒"
                  detail="最近采样区间"
                  icon="arrow"
                />
              </section>
              <div className="overview-grid">
                <section className="panel chart-panel">
                  <div className="panel-header">
                    <div>
                      <h2>数据分发</h2>
                      <span className="muted">
                        最近 30 次采样 · 消息数 / 秒
                      </span>
                    </div>
                    <div className="legend">
                      <span>
                        <i />
                        上游接收
                      </span>
                      <span>
                        <i className="light" />
                        下游分发
                      </span>
                    </div>
                  </div>
                  <Chart samples={samples} />
                  <div className="chart-footer">
                    <span>
                      累计接收 <b>{number(data.metrics.updates)}</b>
                    </span>
                    <span>
                      累计分发 <b>{number(data.metrics.delivered)}</b>
                    </span>
                  </div>
                </section>
                <section className="panel capacity-panel">
                  <div className="panel-header">
                    <h2>上游额度</h2>
                    <span className="pill neutral">合并后</span>
                  </div>
                  <Capacity
                    label="过滤器"
                    used={data.metrics.filters}
                    total={data.service.max_filters}
                  />
                  <Capacity
                    label="账户引用"
                    used={data.metrics.accounts}
                    total={data.service.max_accounts}
                  />
                  <div className="info-box">
                    相同过滤器共享上游数据。账户额度按显式引用次数计算。
                  </div>
                  <div className="small-row">
                    <span>订阅确认级别</span>
                    <code>{data.service.commitment}</code>
                  </div>
                  <div className="small-row">
                    <span>运行时间</span>
                    <span>{duration(data.uptime_seconds)}</span>
                  </div>
                </section>
              </div>
              <section className="panel">
                <div className="panel-header">
                  <div>
                    <h2>用户概览</h2>
                    <span className="muted">访问权限与实时用量</span>
                  </div>
                  <button
                    className="text-button"
                    onClick={() => setPage("users")}
                  >
                    查看全部 <Icon name="arrow" size={16} />
                  </button>
                </div>
                <UserTable
                  users={data.users.slice(0, 5)}
                  onEdit={(user) =>
                    setModal({ type: "user", user, revision: data.revision })
                  }
                  add={add}
                />
              </section>
              <section className="health-strip">
                <div>
                  <span className="dot" />
                  <span>连接与事件</span>
                </div>
                <span>
                  TCP 连接{" "}
                  <b>
                    {number(data.metrics.connections)} /{" "}
                    {number(data.service.max_connections)}
                  </b>
                </span>
                <span>
                  拒绝请求 <b>{number(data.metrics.rejected)}</b>
                </span>
                <span>
                  慢客户端 <b>{number(data.metrics.slow_clients)}</b>
                </span>
                <span>
                  上游中断 <b>{number(data.metrics.upstream_failures)}</b>
                </span>
              </section>
            </>
          )}
          {page === "users" && (
            <section className="panel">
              <div className="panel-header">
                <h2>
                  用户列表 <span className="count">{data.users.length}</span>
                </h2>
                <label className="search">
                  <Icon name="search" size={17} />
                  <input
                    aria-label="搜索用户"
                    placeholder="搜索用户名"
                    value={search}
                    onChange={(e) => setSearch(e.target.value)}
                  />
                </label>
              </div>
              <UserTable
                users={data.users.filter((u) =>
                  u.name.toLowerCase().includes(search.toLowerCase()),
                )}
                add={add}
                onEdit={(user) => {
                  setError("");
                  setModal({ type: "user", user, revision: data.revision });
                }}
                actions={(user) => (
                  <>
                    <button
                      onClick={() =>
                        confirm(
                          "轮换用户 Key",
                          `旧 Key 将立即失效，${user.name} 的现有订阅会断开。新的 Key 仅显示一次。`,
                          () =>
                            mutate(`${userPath(user.name)}/rotate`, "POST", {
                              revision: data.revision,
                            }),
                        )
                      }
                    >
                      轮换 Key
                    </button>
                    <button
                      className="danger-text"
                      onClick={() =>
                        confirm(
                          "删除用户",
                          `删除 ${user.name} 并断开其订阅。此操作不会关闭全局 API Key 验证。`,
                          () =>
                            mutate(userPath(user.name), "DELETE", {
                              revision: data.revision,
                            }),
                          true,
                        )
                      }
                    >
                      删除
                    </button>
                  </>
                )}
              />
            </section>
          )}
          {page === "subscriptions" && (
            <section className="panel">
              <div className="panel-header">
                <div>
                  <h2>
                    活动订阅{" "}
                    <span className="count">{data.metrics.subscriptions}</span>
                  </h2>
                  <span className="muted">
                    每条 Subscribe 流独立计数，包括同一 TCP 连接上的多个订阅
                  </span>
                </div>
              </div>
              {data.subscriptions.length ? (
                <div className="table-wrap">
                  <table>
                    <thead>
                      <tr>
                        <th>用户 / 订阅</th>
                        <th>来源 IP</th>
                        <th>过滤器 / 账户</th>
                        <th>已分发消息</th>
                        <th>建立时间</th>
                        <th>操作</th>
                      </tr>
                    </thead>
                    <tbody>
                      {data.subscriptions.map((sub) => (
                        <tr key={sub.id}>
                          <td>
                            <strong>{sub.user}</strong>
                            <small>#{sub.id}</small>
                          </td>
                          <td>
                            <code>{sub.peer}</code>
                          </td>
                          <td>
                            {sub.filters} / {sub.accounts}
                          </td>
                          <td>{number(sub.delivered)}</td>
                          <td>
                            {new Date(sub.started_at_ms).toLocaleTimeString(
                              "zh-CN",
                            )}
                          </td>
                          <td>
                            <button
                              className="table-button danger-text"
                              onClick={() =>
                                confirm(
                                  "断开订阅",
                                  `断开 ${sub.user} 的订阅 #${sub.id}。用户仍可以在额度内重新连接。`,
                                  () =>
                                    mutate(
                                      `/api/subscriptions/${sub.id}/disconnect`,
                                      "POST",
                                    ),
                                  true,
                                )
                              }
                            >
                              断开
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                  {data.subscriptions.length < data.metrics.subscriptions && (
                    <p className="table-note">
                      展示前 200
                      条订阅。可在用户管理中停用用户以断开其全部订阅。
                    </p>
                  )}
                </div>
              ) : (
                <Empty
                  icon="activity"
                  title="暂无活动订阅"
                  text="用户连接 gRPC 服务并提交过滤器后，订阅会出现在这里。"
                />
              )}
            </section>
          )}
          {page === "access" && (
            <AccessPanel
              key={accessGeneration}
              data={data}
              busy={busy}
              save={(revision, required, ips) =>
                confirm(
                  "应用访问策略",
                  required !== data.require_api_key
                    ? "切换验证方式会断开全部现有订阅。客户端需要按新的策略重新连接。"
                    : "移出白名单的 IP 将无法连接，其现有订阅也会断开。",
                  () =>
                    mutate("/api/access", "PUT", {
                      revision,
                      require_api_key: required,
                      allowed_sources: ips,
                    }),
                )
              }
            />
          )}
          <footer className="page-footer">
            <span>共享 gRPC 接收端 · Shred 路由通过 TOML 管理</span>
            <span>最近同步 {lastSync.toLocaleTimeString("zh-CN")}</span>
          </footer>
        </main>
      </div>
      {modal?.type === "user" && (
        <UserDialog
          user={modal.user}
          defaults={data.service}
          busy={busy}
          error={error}
          close={() => !busy && setModal(undefined)}
          submit={(name, enabled, limits) =>
            mutate(
              modal.user ? userPath(modal.user.name) : "/api/users",
              modal.user ? "PUT" : "POST",
              modal.user
                ? { revision: modal.revision, enabled, limits }
                : { revision: modal.revision, name, limits },
            )
          }
        />
      )}
      {modal?.type === "confirm" && (
        <Dialog title={modal.title} close={() => !busy && setModal(undefined)}>
          <p className="modal-description">{modal.description}</p>
          {error && (
            <div className="alert error" role="alert">
              {error}
            </div>
          )}
          <div className="modal-actions">
            <button
              className="secondary"
              disabled={busy}
              onClick={() => setModal(undefined)}
            >
              取消
            </button>
            <button
              className={modal.danger ? "danger" : "primary"}
              disabled={busy}
              onClick={() => void modal.action()}
            >
              {busy ? "正在应用…" : "确认操作"}
            </button>
          </div>
        </Dialog>
      )}
      {modal?.type === "secret" && (
        <SecretDialog secret={modal.key} close={() => setModal(undefined)} />
      )}
    </div>
  );
}

function Metric({
  title,
  value,
  suffix,
  detail,
  icon,
  positive,
}: {
  title: string;
  value: string;
  suffix?: string;
  detail: string;
  icon: string;
  positive?: boolean;
}) {
  return (
    <article className="metric">
      <div className="metric-title">
        {title}
        <Icon name={icon} />
      </div>
      <div className="metric-value">
        {value}
        {suffix && <span>{suffix}</span>}
      </div>
      <div className={`metric-detail ${positive ? "positive" : ""}`}>
        {positive && <span className="dot" />}
        {detail}
      </div>
    </article>
  );
}
function Capacity({
  label,
  used,
  total,
}: {
  label: string;
  used: number;
  total: number;
}) {
  return (
    <div className="capacity">
      <div>
        <span>{label}</span>
        <code>
          <b>{number(used)}</b> / {number(total)}
        </code>
      </div>
      <progress aria-label={`${label}额度`} value={used} max={total} />
    </div>
  );
}
function Chart({ samples }: { samples: Sample[] }) {
  const max = Math.max(2, ...samples.flatMap((s) => [s.inbound, s.delivered]));
  const line = (key: "inbound" | "delivered") =>
    samples
      .map(
        (sample, i) =>
          `${i ? "L" : "M"}${48 + (i * 600) / Math.max(29, samples.length - 1)},${178 - (sample[key] / max) * 145}`,
      )
      .join(" ");
  return (
    <div className="chart">
      <svg viewBox="0 0 680 218" role="img" aria-label="上游接收与下游分发速率">
        <defs>
          <linearGradient id="area" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor="#a6dbc6" stopOpacity=".4" />
            <stop offset="100%" stopColor="#a6dbc6" stopOpacity="0" />
          </linearGradient>
        </defs>
        {[0, 0.5, 1].map((n) => (
          <g key={n}>
            <line
              x1="48"
              x2="656"
              y1={178 - n * 145}
              y2={178 - n * 145}
              className="grid-line"
            />
            <text x="35" y={182 - n * 145} textAnchor="end">
              {number(Math.round(max * n))}
            </text>
          </g>
        ))}
        {samples.length > 1 && (
          <>
            <path
              d={`${line("delivered")} L${48 + ((samples.length - 1) * 600) / 29},178 L48,178 Z`}
              fill="url(#area)"
            />
            <path d={line("delivered")} className="chart-line light" />
            <path d={line("inbound")} className="chart-line" />
          </>
        )}
        <text x="48" y="207">
          {samples.length
            ? new Date(samples[0].at).toLocaleTimeString("zh-CN")
            : "等待采样"}
        </text>
        <text x="656" y="207" textAnchor="end">
          现在
        </text>
      </svg>
      {samples.length < 2 && (
        <div className="chart-wait">正在采集实时数据…</div>
      )}
    </div>
  );
}
function Empty({
  icon,
  title,
  text,
  children,
}: {
  icon: string;
  title: string;
  text: string;
  children?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="empty-icon">
        <Icon name={icon} size={25} />
      </span>
      <h3>{title}</h3>
      <p>{text}</p>
      {children}
    </div>
  );
}
function UserTable({
  users,
  onEdit,
  actions,
  add,
}: {
  users: User[];
  onEdit: (user: User) => void;
  actions?: (user: User) => ReactNode;
  add: () => void;
}) {
  if (!users.length)
    return (
      <Empty
        icon="users"
        title="这里还没有用户"
        text="创建用户，为每个 Bot 分配独立 Key 和订阅额度。"
      >
        <button className="secondary" onClick={add}>
          <Icon name="plus" size={16} />
          添加用户
        </button>
      </Empty>
    );
  return (
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th>用户</th>
            <th>状态</th>
            <th>订阅用量</th>
            <th>过滤器 / 账户额度</th>
            <th>账户更新</th>
            <th>操作</th>
          </tr>
        </thead>
        <tbody>
          {users.map((user) => (
            <tr key={user.name}>
              <td>
                <div className="user-cell">
                  <span className="user-avatar">
                    {user.name.slice(0, 1).toUpperCase()}
                  </span>
                  <div>
                    <strong>{user.name}</strong>
                    <small>
                      {user.has_key ? "独立 API Key" : "未设置 Key"}
                    </small>
                  </div>
                </div>
              </td>
              <td>
                <span className={`pill ${user.enabled ? "green" : "neutral"}`}>
                  <span className="dot" />
                  {user.enabled ? "已启用" : "已停用"}
                </span>
              </td>
              <td>
                <span className="usage-value">
                  {user.usage.subscriptions}{" "}
                  <span>/ {user.effective.max_subscriptions}</span>
                </span>
                {user.usage.pending > 0 && (
                  <small>{user.usage.pending} 条等待中</small>
                )}
              </td>
              <td>
                <code>
                  {user.effective.max_filters} / {user.effective.max_accounts}
                </code>
              </td>
              <td>
                <span
                  className={`permission ${user.effective.allow_accounts ? "positive" : ""}`}
                >
                  {user.effective.allow_accounts ? "允许" : "关闭"}
                </span>
              </td>
              <td>
                <div className="table-actions">
                  <button onClick={() => onEdit(user)}>编辑</button>
                  {actions?.(user)}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
function Dialog({
  title,
  close,
  children,
}: {
  title: string;
  close: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = ref.current!;
    dialog.showModal();
    return () => dialog.close();
  }, []);
  return (
    <dialog
      ref={ref}
      className="modal"
      aria-labelledby="dialog-title"
      onCancel={(e) => {
        e.preventDefault();
        close();
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget) close();
      }}
    >
      <div className="modal-inner">
        <div className="modal-heading">
          <h2 id="dialog-title">{title}</h2>
          <button
            className="icon-button"
            type="button"
            aria-label="关闭弹窗"
            onClick={close}
          >
            <Icon name="close" />
          </button>
        </div>
        {children}
      </div>
    </dialog>
  );
}
function UserDialog({
  user,
  defaults,
  busy,
  error,
  close,
  submit,
}: {
  user?: User;
  defaults: Snapshot["service"];
  busy: boolean;
  error: string;
  close: () => void;
  submit: (name: string, enabled: boolean, limits: Effective) => Promise<void>;
}) {
  const [localError, setLocalError] = useState("");
  const limits = user?.effective ?? {
    max_subscriptions: defaults.max_subscriptions_per_key,
    max_filters: defaults.max_filters_per_key,
    max_accounts: defaults.max_accounts_per_key,
    allow_accounts: defaults.allow_accounts,
  };
  function save(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setLocalError("");
    try {
      const values = new FormData(e.currentTarget);
      void submit(
        String(values.get("name")),
        values.get("enabled") === "on",
        limitsFromForm(e.currentTarget),
      );
    } catch (e) {
      setLocalError(message(e));
    }
  }
  return (
    <Dialog title={user ? `编辑 ${user.name}` : "添加用户"} close={close}>
      <p className="modal-description">
        {user
          ? "保存后该用户的现有订阅会断开，重新连接时应用新策略。"
          : "为用户生成独立的随机 API Key。Key 将在创建后显示一次。"}
      </p>
      <form onSubmit={save}>
        <label htmlFor="user-name">用户名</label>
        <input
          id="user-name"
          name="name"
          defaultValue={user?.name}
          readOnly={!!user}
          required
          maxLength={64}
          pattern={user ? undefined : "[A-Za-z0-9_.\\-]+"}
          placeholder="例如 trading-bot-01"
        />
        <div className="form-grid">
          {[
            { key: "max_subscriptions", title: "最大并发订阅", max: 65536 },
            { key: "max_filters", title: "过滤器总额度", max: 4096 },
            { key: "max_accounts", title: "账户引用总额度", max: 65536 },
          ].map((field) => (
            <div key={field.key}>
              <label htmlFor={field.key}>{field.title}</label>
              <input
                id={field.key}
                name={field.key}
                type="number"
                min={1}
                max={field.max}
                step={1}
                required
                defaultValue={limits[field.key as keyof Effective] as number}
              />
            </div>
          ))}
        </div>
        <p className="field-help">
          过滤器和账户额度在该用户的所有订阅间累计；上游全局额度同时生效。
        </p>
        <label className="check-card">
          <span>
            <strong>允许账户更新</strong>
            <small>账户更新可能产生大量流量，按用户需求开启。</small>
          </span>
          <input
            type="checkbox"
            name="allow_accounts"
            defaultChecked={limits.allow_accounts}
          />
        </label>
        {user ? (
          <label className="check-card">
            <span>
              <strong>启用用户</strong>
              <small>停用会阻止该身份连接并断开现有订阅。</small>
            </span>
            <input
              type="checkbox"
              name="enabled"
              defaultChecked={user.enabled}
            />
          </label>
        ) : (
          <input name="enabled" type="hidden" value="on" />
        )}
        {(localError || error) && (
          <div role="alert" className="alert error">
            {localError || error}
          </div>
        )}
        <div className="modal-actions">
          <button
            className="secondary"
            type="button"
            disabled={busy}
            onClick={close}
          >
            取消
          </button>
          <button className="primary" disabled={busy}>
            {busy ? "正在保存…" : user ? "保存并应用" : "创建用户"}
          </button>
        </div>
      </form>
    </Dialog>
  );
}
function SecretDialog({
  secret,
  close,
}: {
  secret: string;
  close: () => void;
}) {
  const [copied, setCopied] = useState(false);
  const [visible, setVisible] = useState(false);
  const [copyError, setCopyError] = useState(false);
  return (
    <Dialog title="保存用户 API Key" close={close}>
      <div className="secret-icon">
        <Icon name="key" size={26} />
      </div>
      <p className="modal-description">
        请将此 Key 交给对应的 Bot 用户。关闭后无法再次查看，丢失时需要轮换。
      </p>
      <label htmlFor="new-key">API Key</label>
      <input
        id="new-key"
        className="secret-input"
        type={visible ? "text" : "password"}
        readOnly
        value={secret}
        onFocus={(e) => e.currentTarget.select()}
      />
      <div className="secret-tools">
        <button className="text-button" onClick={() => setVisible(!visible)}>
          {visible ? "隐藏" : "显示"} Key
        </button>
        <button
          className="secondary"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(secret);
              setCopied(true);
              setCopyError(false);
            } catch {
              setVisible(true);
              setCopyError(true);
            }
          }}
        >
          {copied && <Icon name="check" size={16} />}{" "}
          {copied ? "已复制" : "复制 Key"}
        </button>
      </div>
      {copyError && (
        <p role="alert">浏览器未允许复制，请选中上方 Key 手动复制。</p>
      )}
      <div className="info-box">
        客户端使用 api_key 或 x-token 传递此值。它与管理员令牌、供应商 Token
        相互独立。
      </div>
      <div className="modal-actions">
        <button className="primary" onClick={close}>
          我已保存
        </button>
      </div>
    </Dialog>
  );
}
function AccessPanel({
  data,
  busy,
  save,
}: {
  data: Snapshot;
  busy: boolean;
  save: (revision: string, required: boolean, ips: string[]) => void;
}) {
  const [draft, setDraft] = useState({
    revision: data.revision,
    required: data.require_api_key,
    ips: data.allowed_sources.join("\n"),
  });
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!dirty)
      setDraft({
        revision: data.revision,
        required: data.require_api_key,
        ips: data.allowed_sources.join("\n"),
      });
  }, [data.revision, data.require_api_key, data.allowed_sources, dirty]);
  return (
    <div className="settings-grid">
      <section className="panel settings-panel">
        <div className="panel-header">
          <h2>客户端访问控制</h2>
          <span className="pill green">实时应用</span>
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            save(draft.revision, draft.required, [
              ...new Set(
                draft.ips
                  .split(/\r?\n/)
                  .map((s) => s.trim())
                  .filter(Boolean),
              ),
            ]);
          }}
        >
          <label className="check-card">
            <span>
              <strong>要求用户 API Key</strong>
              <small>开启后，Key 与 IP 白名单必须同时通过。</small>
            </span>
            <input
              type="checkbox"
              aria-label="要求用户 API Key"
              checked={draft.required}
              onChange={(e) => {
                setDirty(true);
                setDraft({ ...draft, required: e.target.checked });
              }}
            />
          </label>
          <label htmlFor="allowed-ips">IP 白名单</label>
          <textarea
            id="allowed-ips"
            value={draft.ips}
            onChange={(e) => {
              setDirty(true);
              setDraft({ ...draft, ips: e.target.value });
            }}
            rows={7}
            placeholder={"127.0.0.1\n10.0.0.30"}
          />
          <p className="field-help">
            每行一个 IPv4 或 IPv6 地址，使用客户端实际出口 IP。留空将拒绝所有
            gRPC 客户端，不支持 CIDR。
          </p>
          {!draft.required && (
            <div className="alert warning">
              关闭 Key 验证后，白名单 IP 可直接订阅，用户额度按 IP 计算。
            </div>
          )}
          {dirty && draft.revision !== data.revision && (
            <div className="alert warning">
              服务配置已更新。请先重新载入，再提交修改。
            </div>
          )}
          <div className="modal-actions">
            <button
              className="secondary"
              type="button"
              onClick={() => {
                setDirty(false);
                setDraft({
                  revision: data.revision,
                  required: data.require_api_key,
                  ips: data.allowed_sources.join("\n"),
                });
              }}
            >
              重新载入
            </button>
            <button
              className="primary"
              disabled={busy || !dirty || draft.revision !== data.revision}
            >
              保存访问策略
            </button>
          </div>
        </form>
      </section>
      <section className="panel settings-panel">
        <div className="panel-header">
          <h2>服务配置</h2>
          <span className="pill neutral">TOML 管理</span>
        </div>
        <dl className="config-list">
          <dt>gRPC 监听</dt>
          <dd>
            <code>{data.service.grpc_listen}</code>
          </dd>
          <dt>客户端传输</dt>
          <dd>{data.service.client_tls ? "TLS" : "明文 TCP"}</dd>
          <dt>最大 TCP 连接数</dt>
          <dd>{data.service.max_connections}</dd>
          <dt>最大订阅数</dt>
          <dd>{data.service.max_subscriptions}</dd>
          <dt>上游过滤器 / 账户上限</dt>
          <dd>
            {data.service.max_filters} / {data.service.max_accounts}
          </dd>
          <dt>确认级别</dt>
          <dd>{data.service.commitment}</dd>
          <dt>无账户条件的全量订阅</dt>
          <dd>{data.service.allow_unfiltered ? "允许" : "禁止"}</dd>
        </dl>
        <div className="info-box">
          全局上游额度、监听地址、传输方式与 Shred 路由在 TOML
          中配置，修改后需要重启服务。面板中的用户与访问策略单独持久化。
        </div>
      </section>
    </div>
  );
}
