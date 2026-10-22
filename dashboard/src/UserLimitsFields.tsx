import { useState } from "react";
import type { Limits, Service } from "./api";

type QuotaName = "max_subscriptions" | "max_filters" | "max_accounts";
function QuotaField({
  name,
  title,
  max,
  value,
  fallback,
  disabled,
}: {
  name: QuotaName;
  title: string;
  max: number;
  value: number | null;
  fallback: number;
  disabled: boolean;
}) {
  const [inherited, setInherited] = useState(value === null);
  const [custom, setCustom] = useState(String(value ?? fallback));
  return (
    <div>
      <label htmlFor={name}>{title}</label>
      <input
        id={name}
        name={name}
        type="number"
        min={1}
        max={max}
        step={1}
        required
        disabled={disabled || inherited}
        value={inherited ? fallback : custom}
        onChange={(event) => setCustom(event.target.value)}
      />
      <label className="inherit-option">
        <input
          type="checkbox"
          name={`${name}_inherit`}
          checked={inherited}
          disabled={disabled}
          aria-label={`${title}跟随服务默认`}
          onChange={(event) => setInherited(event.target.checked)}
        />
        跟随默认（{fallback}）
      </label>
    </div>
  );
}

export function UserLimitsFields({
  limits,
  defaults,
  disabled,
}: {
  limits?: Limits;
  defaults: Service;
  disabled: boolean;
}) {
  const [inheritAccounts, setInheritAccounts] = useState(
    (limits?.allow_accounts ?? null) === null,
  );
  const [allowAccounts, setAllowAccounts] = useState(
    limits?.allow_accounts ?? defaults.allow_accounts,
  );
  return (
    <>
      <div className="form-grid">
        <QuotaField
          name="max_subscriptions"
          title="最大并发订阅"
          max={65536}
          value={limits?.max_subscriptions ?? null}
          fallback={defaults.max_subscriptions_per_key}
          disabled={disabled}
        />
        <QuotaField
          name="max_filters"
          title="过滤器总额度"
          max={4096}
          value={limits?.max_filters ?? null}
          fallback={defaults.max_filters_per_key}
          disabled={disabled}
        />
        <QuotaField
          name="max_accounts"
          title="账户引用总额度"
          max={65536}
          value={limits?.max_accounts ?? null}
          fallback={defaults.max_accounts_per_key}
          disabled={disabled}
        />
      </div>
      <p className="field-help">
        跟随默认会保留单条订阅和用户累计的默认限制。自定义过滤器或账户额度同时用于单条和累计限制，上游全局额度始终生效。
      </p>
      <label className="check-card">
        <span>
          <strong>允许账户更新</strong>
          <small>账户更新可能产生大量流量，按用户需求开启。</small>
        </span>
        <input
          type="checkbox"
          name="allow_accounts"
          disabled={disabled || inheritAccounts}
          checked={inheritAccounts ? defaults.allow_accounts : allowAccounts}
          onChange={(event) => setAllowAccounts(event.target.checked)}
        />
      </label>
      <label className="inherit-option">
        <input
          type="checkbox"
          name="allow_accounts_inherit"
          checked={inheritAccounts}
          disabled={disabled}
          aria-label="账户更新权限跟随服务默认"
          onChange={(event) => setInheritAccounts(event.target.checked)}
        />
        账户权限跟随默认（{defaults.allow_accounts ? "允许" : "关闭"}）
      </label>
    </>
  );
}
