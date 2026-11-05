import { useDeferredValue, useEffect, useRef, useState } from "react";
import { fetchGpuDocument, GPU_DOCUMENT_URL, saveGpuDocument, type MarkdownDocument } from "../api/documents";
import { MarkdownPreview } from "../components/MarkdownPreview";
import { PageHeading } from "../components/PageHeading";
import { formatDate } from "../lib/format";
import { downloadMarkdown, GPU_MARKDOWN_FILENAME, readMarkdownFile, validateMarkdown } from "../lib/markdownDocument";
import "./GpuComparison.css";

export default function GpuComparison({ onDirtyChange }: { onDirtyChange?: (dirty: boolean) => void }) {
  const [data, setData] = useState<MarkdownDocument | null>(null);
  const [draft, setDraft] = useState("");
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [message, setMessage] = useState("");
  const input = useRef<HTMLInputElement>(null);
  const operation = useRef<AbortController | null>(null);
  const active = useRef(true);
  const dirty = data !== null && draft !== data.content;
  const preview = useDeferredValue(draft);
  const invalid = validateMarkdown(draft, data?.max_bytes);

  useEffect(() => { onDirtyChange?.(dirty); }, [dirty, onDirtyChange]);
  useEffect(() => () => { onDirtyChange?.(false); }, [onDirtyChange]);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); event.returnValue = ""; };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [dirty]);

  function begin() {
    const controller = new AbortController();
    operation.current = controller; setBusy(true); setError(""); setMessage("");
    return { controller };
  }
  function finish(controller: AbortController) {
    if (operation.current === controller) { operation.current = null; if (active.current) setBusy(false); }
  }
  async function load() {
    if (operation.current) return;
    if (dirty && !window.confirm("重新读取会丢弃当前未保存的草稿，确认继续？可先下载草稿。")) return;
    const { controller } = begin();
    try {
      const document = await fetchGpuDocument(AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]));
      if (active.current) { setData(document); setDraft(document.content); setEditing(false); }
    } catch (caught) { if (active.current && !controller.signal.aborted) setError(caught instanceof Error ? caught.message : "文档读取失败"); }
    finally { finish(controller); }
  }
  useEffect(() => {
    active.current = true;
    const timer = window.setTimeout(() => void load(), 0);
    return () => { active.current = false; window.clearTimeout(timer); operation.current?.abort(); operation.current = null; };
    // One initial request; editing and page visibility never start polling.
  }, []);

  async function persist(content: string, source: "edit" | "upload", reserved?: ReturnType<typeof begin>) {
    if (!data || (!reserved && operation.current)) return;
    const invalid = validateMarkdown(content, data.max_bytes);
    if (invalid) { setError(invalid); return; }
    const { controller } = reserved ?? begin();
    try {
      const document = await saveGpuDocument(content, data.revision, source, AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]));
      if (active.current) {
        setData(document); setDraft(document.content); setEditing(false);
        setMessage(source === "upload" ? "Markdown 已上传并覆盖，所有浏览器共享新版本。" : "文档已保存。");
      }
    } catch (caught) { if (active.current && !controller.signal.aborted) setError(caught instanceof Error ? caught.message : "保存失败，草稿已保留"); }
    finally { finish(controller); }
  }
  async function upload(file: File) {
    if (!data || operation.current) return;
    const reserved = begin();
    try {
      const content = await readMarkdownFile(file);
      if (!active.current || reserved.controller.signal.aborted) return;
      if (!window.confirm(`确认用“${file.name}”整体覆盖平台共享文档？${dirty ? "当前未保存的草稿也会替换。" : ""}`)) return;
      // Failed/concurrent uploads keep the selected file as a downloadable draft.
      setDraft(content); setEditing(true);
      await persist(content, "upload", reserved);
    } catch (caught) { if (active.current) setError(caught instanceof Error ? caught.message : "Markdown 上传失败"); }
    finally { finish(reserved.controller); }
  }
  function cancel() {
    if (!data || (dirty && !window.confirm("放弃当前未保存的修改？"))) return;
    setDraft(data.content); setEditing(false); setError(""); setMessage("");
  }

  return <>
    <PageHeading eyebrow="GPU reference · Markdown" title="GPU 性能对比" description="GPU 参数、计算场景与选型参考 · 在线阅读、编辑和管理 Markdown" />
    <section className="page-content gpu-document">
      <article className="data-panel">
        <header className="panel-header"><div><h2>{editing ? "编辑 Markdown" : "GPU 性能参考"}</h2>
          <p>{data ? `${data.revision === 0 ? "初始文档" : `版本 ${data.revision}`} · ${data.updated_at ? `保存于 ${formatDate(data.updated_at)}` : "已导入原始 Markdown"}` : "正在读取文档…"}{dirty && " · 有未保存的修改"}</p>
        </div><div className="gpu-document__toolbar">
          <button type="button" className="button button--secondary button--small" disabled={busy} onClick={() => void load()}>重新读取</button>
          {data && <>
            {!editing && <button type="button" className="button button--small" disabled={busy} onClick={() => { setEditing(true); setMessage(""); }}>编辑 Markdown</button>}
            {editing && <><button type="button" className="button button--small" disabled={busy || !dirty || !!invalid} onClick={() => void persist(draft, "edit")}>{busy ? "保存中…" : "保存"}</button>
              <button type="button" className="button button--secondary button--small" disabled={busy} onClick={cancel}>取消编辑</button>
              <button type="button" className="button button--secondary button--small" onClick={() => downloadMarkdown(draft, `草稿_${GPU_MARKDOWN_FILENAME}`)}>下载草稿</button></>}
            <button type="button" className="button button--secondary button--small" disabled={busy} onClick={() => input.current?.click()}>上传 Markdown（覆盖）</button>
            <a className="button button--secondary button--small" href={`${GPU_DOCUMENT_URL}/download`} download>下载 Markdown</a>
          </>}
          <input ref={input} type="file" hidden accept=".md,.markdown,text/markdown" disabled={busy || !data} aria-label="上传 Markdown 覆盖文档"
            onChange={(event) => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ""; if (file) void upload(file); }} />
        </div></header>
        {error && <p className="gpu-document__notice form-error" role="alert">{error}</p>}
        {message && <p className="gpu-document__notice positive" role="status">{message}</p>}
        {busy && !data && <p className="table-empty" role="status">正在读取 Markdown…</p>}
        {data && <>
          {editing ? <div className="gpu-document__editor">
            <label><span>Markdown 源码</span><textarea value={draft} spellCheck={false} disabled={busy} aria-label="GPU 性能对比 Markdown 源码"
              onChange={(event) => { setDraft(event.target.value); setError(""); setMessage(""); }}
              onKeyDown={(event) => { if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s") { event.preventDefault(); if (!busy && dirty && !invalid) void persist(draft, "edit"); } }} /></label>
            <section aria-label="Markdown 实时预览"><h3>实时预览</h3><MarkdownPreview content={preview} /></section>
          </div> : <div className="gpu-document__reader"><MarkdownPreview content={data.content} /></div>}
          <p className="hint gpu-document__footer">保存和上传覆盖会更新平台共享文档；上传支持 UTF-8 .md / .markdown，最大 1 MiB。下载 Markdown 获取已保存版本，下载草稿保留当前编辑内容。{editing && <> · {draft.length.toLocaleString()} 个字符{invalid && <span className="form-error"> · {invalid}</span>}</>}</p>
        </>}
      </article>
    </section>
  </>;
}
