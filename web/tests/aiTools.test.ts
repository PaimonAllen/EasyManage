// Run only in the documented isolated test environment.
import assert from "node:assert/strict";
import { test } from "node:test";
import { agentToolLabel, isAgentTool } from "../src/lib/aiTools.ts";
import type { AiToolStatus } from "../src/types/monitoring";

const tool = (name: string, category = "assistant"): AiToolStatus => ({
  name, category, executable_paths: [], processes: [], service_units: [],
});

test("AI tools means named Agent tools, not runtimes, providers or programming environments", () => {
  for (const name of ["codex", "claude", "gemini", "opencode", "aider"]) {
    assert.ok(isAgentTool(tool(name)));
  }
  for (const name of ["ollama", "vllm", "llama-server", "litellm", "open-webui", "comfyui", "lms", "python3", "nvcc", "anthropic", "unknown", "toString", "__proto__"]) {
    assert.equal(isAgentTool(tool(name)), false);
  }
});

test("legacy mixed inventory is filtered consistently and CLI identities remain compatible", () => {
  const legacy = [tool("claude"), tool("codex"), tool("ollama", "runtime"), tool("vllm", "runtime")];
  assert.deepEqual(legacy.filter(isAgentTool).map(item => item.name), ["claude", "codex"]);
  assert.equal(agentToolLabel("claude"), "Claude Code");
  assert.equal(agentToolLabel("gemini"), "Gemini CLI");
  assert.equal(agentToolLabel("unknown"), "unknown");
});
