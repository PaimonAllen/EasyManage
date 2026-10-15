# 只读诊断

`inspect_disk_configuration.sh` 查看块设备和文件系统；`inspect_port80_nginx.sh` 查看既有 Nginx 80 端口配置和本机 HTTP 响应。不修改磁盘、分区、权限或服务。

普通用户不可读的配置由管理员明确执行；输出可能含主机地址、业务目录及 Nginx 配置，审阅后分享，不保存到公开仓库。
