import { useState, type FormEvent, type InputHTMLAttributes } from "react";
import { request } from "./api";

export function PasswordInput({
  label,
  ...props
}: InputHTMLAttributes<HTMLInputElement> & { label: string }) {
  const [visible, setVisible] = useState(false);
  return (
    <div className="password-input-row">
      <input {...props} type={visible ? "text" : "password"} />
      <button
        type="button"
        className="password-toggle"
        aria-label={`${visible ? "隐藏" : "显示"}${label}`}
        aria-pressed={visible}
        onClick={() => setVisible(!visible)}
      >
        {visible ? "隐藏" : "显示"}
      </button>
    </div>
  );
}

export function PasswordForm({
  token,
  onChanged,
  onWorking,
}: {
  token: string;
  onChanged: () => void;
  onWorking?: (value: boolean) => void;
}) {
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    setError("");
    const form = e.currentTarget;
    const values = new FormData(form);
    const current_password = String(values.get("current_password") ?? "");
    const new_password = String(values.get("new_password") ?? "");
    if (new_password !== values.get("confirmation")) {
      setError("两次输入的新密码不一致。");
      return;
    }
    if (
      [...new_password].length < 12 ||
      new TextEncoder().encode(new_password).length > 128 ||
      !new_password.trim()
    ) {
      setError("新密码至少 12 个字符，最多 128 个 UTF-8 字节。");
      return;
    }
    if (current_password === new_password) {
      setError("新密码不能与当前密码相同。");
      return;
    }
    setBusy(true);
    onWorking?.(true);
    try {
      await request(token, "/api/auth/password", "POST", {
        current_password,
        new_password,
      });
      form.reset();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : "修改密码失败，请稍后重试。");
    } finally {
      setBusy(false);
      onWorking?.(false);
    }
  }
  return (
    <form onSubmit={submit} className="password-form">
      <label htmlFor="current-password">当前密码</label>
      <PasswordInput
        label="当前密码"
        id="current-password"
        name="current_password"
        type="password"
        autoComplete="current-password"
        required
        maxLength={128}
      />
      <label htmlFor="new-password">新密码</label>
      <PasswordInput
        label="新密码"
        id="new-password"
        name="new_password"
        type="password"
        autoComplete="new-password"
        required
        minLength={12}
        maxLength={128}
      />
      <label htmlFor="confirm-password">确认新密码</label>
      <PasswordInput
        label="确认新密码"
        id="confirm-password"
        name="confirmation"
        type="password"
        autoComplete="new-password"
        required
        minLength={12}
        maxLength={128}
      />
      <p className="field-help">
        至少 12 个字符。修改成功后，所有 Dashboard
        登录会话都会失效，需要重新登录。Bot 用户的 API Key 不受影响。
      </p>
      {error && (
        <div role="alert" className="alert error">
          {error}
        </div>
      )}
      <button className="primary full" disabled={busy}>
        {busy ? "正在修改…" : "保存新密码"}
      </button>
    </form>
  );
}
