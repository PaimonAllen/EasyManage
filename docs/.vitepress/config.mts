import { defineConfig } from "vitepress";

export default defineConfig({
  lang: "zh-CN",
  title: "EasyManage",
  description: "裸机基础设施监控与运维平台文档",
  base: "/guide/",
  srcExclude: ["README.md", "design/**"],
  cleanUrls: false,
  themeConfig: {
    nav: [
      { text: "操作手册", link: "/" },
      { text: "平台文档", link: "/easymanage" },
      { text: "部署", link: "/deployment" },
      { text: "监控", link: "/monitoring" },
      { text: "编程环境", link: "/programming" },
    ],
    sidebar: [
      { text: "服务器手册", items: [{ text: "原操作手册", link: "/" }] },
      { text: "EasyManage 平台", items: [{ text: "平台概览", link: "/easymanage" }, { text: "平台结构", link: "/platform" }, { text: "架构与跨平台", link: "/architecture" }] },
      { text: "运维", items: [{ text: "裸机部署", link: "/deployment" }, { text: "监控与权限", link: "/monitoring" }] },
      { text: "清单", items: [{ text: "编程环境", link: "/programming" }] },
    ],
    search: { provider: "local" },
    footer: { message: "EasyManage · 内网裸机基础设施平台" },
  },
});
