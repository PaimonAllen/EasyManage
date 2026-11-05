import assert from "node:assert/strict";
import { test } from "node:test";
import { MARKDOWN_MAX_BYTES, readMarkdownFile, validateMarkdown } from "../src/lib/markdownDocument.ts";
import { fetchGpuDocument, saveGpuDocument } from "../src/api/documents.ts";

test("Markdown uploads preserve UTF-8 and line endings, while rejecting binary/invalid or oversized files", async () => {
  const encode=(value:string)=>new TextEncoder().encode(value).buffer;
  const content="# GPU 中文\r\n\n| GPU | 显存 |\n|---|---|\n| V100 | 32 GB |\n";
  assert.equal(await readMarkdownFile({name:"GPU.MD",size:encode(content).byteLength,arrayBuffer:async()=>encode(content)}),content);
  assert.equal(validateMarkdown("中".repeat(MARKDOWN_MAX_BYTES/3+1)),"Markdown 文件最大 1 MiB。");
  assert.ok(validateMarkdown("text\0binary"));
  assert.equal(validateMarkdown("\t# a\r\n"),null);
  await assert.rejects(readMarkdownFile({name:"bad.md",size:2,arrayBuffer:async()=>new Uint8Array([0xff,0xfe]).buffer}),/UTF-8/);
  await assert.rejects(readMarkdownFile({name:"page.html",size:1,arrayBuffer:async()=>encode("# x")}),/\.md/);
  await assert.rejects(readMarkdownFile({name:"large.md",size:MARKDOWN_MAX_BYTES+1,arrayBuffer:async()=>{throw new Error("must not read oversized data");}}),/1 MiB/);
});

test("document writes carry the exact loaded revision; conflicts retain the server message", async () => {
  const original=globalThis.fetch;
  const calls:{url:string;init:RequestInit|undefined}[]=[];
  try {
    globalThis.fetch=async(input,init)=>{
      calls.push({url:String(input),init});
      return new Response(JSON.stringify(init?.method==="PUT"?{message:"version conflict; keep draft"}:{content:"# original",revision:7,updated_at:null,max_bytes:MARKDOWN_MAX_BYTES}),{status:init?.method==="PUT"?409:200,headers:{"Content-Type":"application/json"}});
    };
    assert.equal((await fetchGpuDocument()).revision,7);
    await assert.rejects(saveGpuDocument("# upload",7,"upload"),/keep draft/);
    assert.deepEqual(JSON.parse(calls[1].init?.body as string),{content:"# upload",expected_revision:7,source:"upload"});
    assert.equal(calls[1].url,"/api/v1/documents/gpu-comparison");
  } finally {globalThis.fetch=original;}
});
