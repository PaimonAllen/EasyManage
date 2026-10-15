import { useState } from "react";
import { PageHeading } from "../components/PageHeading";

export function Documentation({
  url,
}: {
  url: string;
}) {
  const [loaded, setLoaded] = useState(false);
  const safeUrl =
    url.startsWith("/") && !url.startsWith("//") ? url : "/guide/easymanage.html";
  return (
    <>
      <PageHeading
        eyebrow="Knowledge base"
        title="服务器操作手册"
        description="原有《服务器管理操作手册》 · 已迁入 VitePress"
      >
        <a className="button button--secondary" href={safeUrl} target="_blank" rel="noopener noreferrer">EasyManage 平台文档 ↗</a>
      </PageHeading>
      <section className="page-content docs-content">
        <p className="hint">
          下方是已迁入 VitePress 的原操作手册；EasyManage 平台文档请使用右侧按钮独立打开。原 Nginx 站点仍保留，但不再用于此页面。
        </p>
        {!loaded && <p className="muted">正在载入文档…</p>}
        <iframe
          className="documentation-frame"
          src="/guide/"
          title="VitePress 服务器管理操作手册"
          onLoad={() => setLoaded(true)}
          referrerPolicy="same-origin"
        />
      </section>
    </>
  );
}
