import { isIP } from "node:net";

export interface ShredConfig {
  listen: string;
  allowed_sources?: string[];
  receive_buffer?: number;
  channel_capacity?: number;
}
export interface YellowstoneConfig {
  endpoint: string;
  x_token_env?: string;
  api_key?: string;
  api_key_env?: string;
  tls_domain?: string;
  ca_file?: string;
  transaction_accounts?: string[];
}
export interface ClientConfig { shred: ShredConfig; yellowstone: YellowstoneConfig }

function object(value: unknown, keys: string[], name: string): asserts value is Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new Error(`${name} must be an object`);
  for (const key of Object.keys(value)) {
    if (!keys.includes(key)) throw new Error(`Unknown config key: ${name}.${key}`);
  }
}
function string(value: unknown, name: string): asserts value is string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${name} must be a nonempty string`);
}
function strings(value: unknown, name: string): asserts value is string[] {
  if (!Array.isArray(value) || !value.every(item => typeof item === "string")) throw new Error(`${name} must be an array of strings`);
}
export function validateShredConfig(value: unknown): asserts value is ShredConfig {
  object(value, ["listen", "allowed_sources", "receive_buffer", "channel_capacity"], "shred");
  string(value.listen, "shred.listen");
  if (value.allowed_sources !== undefined) {
    strings(value.allowed_sources, "shred.allowed_sources");
    for (const ip of value.allowed_sources) {
      if (!isIP(ip)) throw new Error(`Invalid allowed IP: ${ip}`);
    }
  }
  for (const key of ["receive_buffer", "channel_capacity"]) {
    const size = value[key];
    if (size !== undefined && (typeof size !== "number" || !Number.isSafeInteger(size) || size < 1)) throw new Error(`shred.${key} must be a positive integer`);
  }
}
export function validateYellowstoneConfig(value: unknown): asserts value is YellowstoneConfig {
  object(value, ["endpoint", "x_token_env", "api_key", "api_key_env", "tls_domain", "ca_file", "transaction_accounts"], "yellowstone");
  string(value.endpoint, "yellowstone.endpoint");
  for (const key of ["x_token_env", "tls_domain", "ca_file"]) {
    if (value[key] !== undefined) string(value[key], `yellowstone.${key}`);
  }
  for (const key of ["api_key", "api_key_env"]) {
    if (value[key] !== undefined && typeof value[key] !== "string") throw new Error(`yellowstone.${key} must be a string`);
  }
  if (value.api_key && value.api_key_env) throw new Error("set api_key or api_key_env, not both");
  if (typeof value.api_key === "string") validateApiKey(value.api_key);
  if (value.transaction_accounts !== undefined) strings(value.transaction_accounts, "yellowstone.transaction_accounts");
}

/** Validate parsed TOML/JSON before any listeners or connections are opened. */
export function parseClientConfig(value: unknown): ClientConfig {
  object(value, ["shred", "yellowstone"], "config");
  validateShredConfig(value.shred);
  validateYellowstoneConfig(value.yellowstone);
  return { shred: value.shred, yellowstone: value.yellowstone };
}

function validateApiKey(key: string): void {
  if (!/^[\x21-\x7e]{0,256}$/.test(key)) throw new Error("API key must contain at most 256 visible ASCII bytes");
}
export function resolveApiKey(config: YellowstoneConfig): string {
  const key = config.api_key_env ? process.env[config.api_key_env] : (config.api_key ?? "");
  if (key === undefined) throw new Error("Missing API key environment variable");
  validateApiKey(key);
  return key;
}
