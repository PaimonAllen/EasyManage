---
layout: home
hero:
  name: EasyManage
  text: 裸机基础设施运维平台
  tagline: 服务器、Agent 与真实主机指标的统一入口
  actions:
    - theme: brand
      text: 了解平台
      link: /platform
    - theme: alt
      text: 部署说明
      link: /deployment
features:
  - title: 双节点监控
    details: 本机作为控制端兼节点 1，服务器 2 通过可信普通用户 SSH 管理；Agent 通过 HTTPS 上报指标。
  - title: 实际采集
    details: 查看 CPU、内存、磁盘、GPU、网络、用户、软件、代理及开发工具的可见状态；未知数据不作猜测。
  - title: 权限边界
    details: 系统级 Full 与普通用户 / Basic 模式分离；提权、账户管理和在线运维需要目标机实际管理员凭据。
---

## 控制台

本页是 VitePress 构建的 EasyManage 平台文档。监控和运维操作可返回 <a href="/#/overview" target="_top">控制台首页</a>。

平台当前不使用 Docker。服务器 1 和服务器 2 均为生产环境，文档中的部署命令会明确说明是否重启 EasyManage 服务。
