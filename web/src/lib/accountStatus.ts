export function accountState(status: string | null | undefined) {
  switch (status) {
    case "normal": return { label: "正常", className: "positive" };
    case "restricted": return { label: "限制交互", className: "account-status--restricted" };
    case "invalid": return { label: "Shell 异常", className: "form-error" };
    default: return { label: "待核验", className: "muted" };
  }
}
