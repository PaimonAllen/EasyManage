# 编程环境

控制台的“编程环境”页按受管节点展示 Rust、C/C++、GPU、MATLAB、C++(GPU)、Python、JavaScript、Go、Java、构建工具和通用开发工具。页面顶部直接列出单选类别标签，点击后仅显示该类别；“全部”恢复所有类别。已知工具按名称正确分类，兼容旧版 Agent 的通用/C++ 类别。数据来自 Agent 每分钟更新的只读清单；可见的控制台页面每 30 秒刷新显示。

Agent 检查 `/usr/local/bin`、`/usr/bin`、`/bin` 以及其运行用户主目录下的 `.local/bin`、`.npm-global/bin`、`.cargo/bin`。GPU 类工具另外检查 `/usr/local/cuda/bin`、`/opt/rocm/bin`、`/opt/rocm/hip/bin`，以及 `/usr/local`、`/opt` 下符合 `cuda-数字版本`、`rocm-数字版本` 的安装目录。MATLAB 类检查 `/usr/local/MATLAB`、`/opt/MATLAB` 及其小写形式下的 `bin` 和 `R年a/b/bin`。目录发现仅扫描单层，每个根目录最多读取 128 项并保留 16 个版本，不递归遍历文件系统。不会执行发现的程序，也不会读取配置、环境变量、口令或 API 凭据。

| 类别 | 检测工具 | 不代表什么 |
| --- | --- | --- |
| GPU | nvidia-smi、rocm-smi、rocminfo、clinfo | 不代表驱动健康、设备可用或计算验证通过 |
| MATLAB | matlab、mex、mcc | 不代表许可证、工具箱或编译能力已验证 |
| C++(GPU) | nvcc、cuda-gdb、compute-sanitizer、hipcc、hipconfig、amdclang++ | 不代表工具链兼容或 GPU 程序可运行 |

新标签始终可见，但旧版 Agent 没有对应检测项时，页面提示更新 Agent，不误报为未安装。更新后等待下一次清单采集即可；可选择“全部检测项”查看未发现的工具。不会因此安装 CUDA、MATLAB 或占用 GPU 执行测试。

若系统包清单中有对应条目，页面显示系统包版本；用户自行安装的工具版本显示“未知”。“未在限定路径发现”不代表主机未安装该工具。Basic Agent 对其他用户的主目录不做扫描。
