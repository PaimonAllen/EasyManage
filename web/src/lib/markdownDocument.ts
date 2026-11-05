export const MARKDOWN_MAX_BYTES = 1024 * 1024;
export const GPU_MARKDOWN_FILENAME = "GPU_性能对比_8款.md";

export function validateMarkdown(content: string, maxBytes = MARKDOWN_MAX_BYTES): string | null {
  if (new TextEncoder().encode(content).byteLength > maxBytes) return "Markdown 文件最大 1 MiB。";
  if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]/u.test(content)) return "请选择 UTF-8 Markdown 文本，文件不能包含二进制控制字符。";
  return null;
}

export async function readMarkdownFile(file: Pick<File, "name" | "size" | "arrayBuffer">): Promise<string> {
  if (!/\.(?:md|markdown)$/i.test(file.name)) throw new Error("请选择 .md 或 .markdown 文件。");
  if (file.size > MARKDOWN_MAX_BYTES) throw new Error("Markdown 文件最大 1 MiB。");
  let content: string;
  try { content = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer()); }
  catch { throw new Error("文件不是有效的 UTF-8 文本，请转换编码后重试。"); }
  const error = validateMarkdown(content);
  if (error) throw new Error(error);
  return content;
}

export function downloadMarkdown(content: string, filename = GPU_MARKDOWN_FILENAME) {
  const url = URL.createObjectURL(new Blob([content], { type: "text/markdown;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url; link.download = filename;
  document.body.append(link); link.click(); link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
}
