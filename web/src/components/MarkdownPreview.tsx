import { memo } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

// Markdown stays text/AST, never executable HTML. Keep tables usable on narrow
// screens and links in another tab so they cannot discard an editor draft.
export const MarkdownPreview = memo(function MarkdownPreview({ content }: { content: string }) {
  return <div className="markdown-preview">
    {content.trim() ? <Markdown remarkPlugins={[remarkGfm]} skipHtml components={{
      table: ({ node: _node, ...props }) => <div className="markdown-table-scroll"><table {...props} /></div>,
      a: ({ node: _node, href, children, ...props }) => href
        ? <a {...props} href={href} target="_blank" rel="noopener noreferrer">{children}</a> : <span>{children}</span>,
      img: ({ node: _node, ...props }) => props.src
        ? <img {...props} loading="lazy" decoding="async" referrerPolicy="no-referrer" /> : <span>{props.alt}</span>,
    }}>{content}</Markdown> : <p className="muted">文档为空。</p>}
  </div>;
});
