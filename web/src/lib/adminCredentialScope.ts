export class AdminCredentialScopeError extends Error {
  readonly nodeId: string | null;

  constructor(nodeId: string | null, message: string) {
    super(message);
    this.name = "AdminCredentialScopeError";
    this.nodeId = nodeId;
  }
}

// Confirm each selected machine's HttpOnly browser authorization after saving.
// A partial save or an ignored cookie must never authorize a batch update.
export async function verifyAdminCredentialScopes(
  nodeIds: (string | null)[],
  save: (nodeId: string | null) => Promise<unknown>,
  status: (nodeId: string | null) => Promise<{available: boolean}>,
): Promise<void> {
  for (const nodeId of [...new Set(nodeIds)]) {
    try {
      await save(nodeId);
      if (!(await status(nodeId)).available) {
        throw new Error("SSH/sudo 验证后浏览器授权未生效，请确认在同一 HTTPS 地址操作且浏览器允许本站 Cookie；本次未开始更新");
      }
    } catch (error) {
      throw new AdminCredentialScopeError(nodeId, error instanceof Error ? error.message : "管理员鉴权未完成");
    }
  }
}
