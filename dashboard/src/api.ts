export interface LoginResult {
  token: string;
  username: string;
  must_change_password: boolean;
  expires_in_seconds: number;
}
export interface Limits {
  max_subscriptions: number | null;
  max_filters: number | null;
  max_accounts: number | null;
  allow_accounts: boolean | null;
}
export interface Effective {
  max_subscriptions: number;
  max_filters: number;
  max_accounts: number;
  allow_accounts: boolean;
}
export interface User {
  name: string;
  enabled: boolean;
  has_key: boolean;
  limits: Limits;
  effective: Effective;
  usage: {
    subscriptions: number;
    pending: number;
    filters: number;
    accounts: number;
  };
}
export interface Subscription {
  id: string;
  user: string;
  peer: string;
  started_at_ms: number;
  delivered: number;
  filters: number;
  accounts: number;
}
export interface Metrics {
  connections: number;
  subscriptions: number;
  upstream_connections: number;
  upstream_streams: number;
  filters: number;
  accounts: number;
  rejected: number;
  slow_clients: number;
  upstream_failures: number;
  updates: number;
  delivered: number;
}
export interface Service {
  version: string;
  grpc_listen: string;
  commitment: string;
  client_tls: boolean;
  max_connections: number;
  max_subscriptions: number;
  max_filters: number;
  max_accounts: number;
  allow_unfiltered: boolean;
  max_subscriptions_per_key: number;
  max_filters_per_key: number;
  max_accounts_per_key: number;
  allow_accounts: boolean;
}
export interface Snapshot {
  revision: string;
  require_api_key: boolean;
  allowed_sources: string[];
  users: User[];
  subscriptions: Subscription[];
  metrics: Metrics;
  service: Service;
  uptime_seconds: number;
}
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function request<T>(
  token: string,
  path: string,
  method = "GET",
  body?: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: {
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: signal
      ? AbortSignal.any([signal, AbortSignal.timeout(12000)])
      : AbortSignal.timeout(12000),
    cache: "no-store",
    credentials: "omit",
  });
  if (!response.ok) {
    const data = (await response.json().catch(() => null)) as {
      error?: string;
    } | null;
    throw new ApiError(
      response.status,
      data?.error || `请求失败 (${response.status})`,
    );
  }
  return response.json() as Promise<T>;
}
export function limitsFromForm(form: HTMLFormElement): Limits {
  const data = new FormData(form);
  const number = (name: string) => {
    if (data.get(`${name}_inherit`) === "on") return null;
    const value = String(data.get(name));
    if (!/^\d+$/.test(value) || Number(value) < 1 || Number(value) > 65536)
      throw new Error("额度必须是 1–65536 之间的整数。");
    return Number(value);
  };
  const filters = number("max_filters");
  if (filters !== null && filters > 4096)
    throw new Error("过滤器额度最多为 4096。");
  return {
    max_subscriptions: number("max_subscriptions"),
    max_filters: filters,
    max_accounts: number("max_accounts"),
    allow_accounts:
      data.get("allow_accounts_inherit") === "on"
        ? null
        : data.get("allow_accounts") === "on",
  };
}
