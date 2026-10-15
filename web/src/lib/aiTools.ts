import type { AiToolStatus } from "../types/monitoring";

const agentLabels: Record<string, string> = {
  codex: "Codex",
  claude: "Claude Code",
  gemini: "Gemini CLI",
  opencode: "OpenCode",
  aider: "Aider",
};

// Older Agents also report runtimes/WebUIs in ai_tools. Restrict both the table
// and summary counts to named Agent tools, without depending on legacy category.
export function isAgentTool(tool: AiToolStatus): boolean {
  return Object.hasOwn(agentLabels, tool.name);
}

export function agentToolLabel(name: string): string {
  return Object.hasOwn(agentLabels, name) ? agentLabels[name] : name;
}
