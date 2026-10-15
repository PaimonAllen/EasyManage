import type { ProgrammingToolStatus } from "../types/monitoring.ts";

export const PROGRAMMING_CATEGORIES = ["Rust", "C/C++", "GPU", "MATLAB", "C++(GPU)", "Python", "JavaScript", "Go", "Java", "构建工具", "通用工具"];

export const PROGRAMMING_CATEGORY_HINTS: Record<string, string> = {
  GPU: "检测 NVIDIA / ROCm / OpenCL 工具的安装路径；不执行 GPU 探测或计算，不代表驱动与计算环境已验证。",
  MATLAB: "检测 MATLAB、mex、mcc 的安装路径；不启动 MATLAB，不验证许可证、工具箱或编译能力。",
  "C++(GPU)": "检测 CUDA / HIP 编译器与调试工具；不编译或运行 GPU 程序，不代表工具链兼容性已验证。",
};

export function displayProgrammingCategory(tool: ProgrammingToolStatus): string {
  if (tool.name === "go") return "Go";
  if (tool.name === "java" || tool.name === "javac") return "Java";
  if (tool.name === "cmake" || tool.name === "make") return "构建工具";
  if (["nvidia-smi", "rocm-smi", "rocminfo", "clinfo"].includes(tool.name)) return "GPU";
  if (["matlab", "mex", "mcc"].includes(tool.name)) return "MATLAB";
  if (["nvcc", "cuda-gdb", "compute-sanitizer", "hipcc", "hipconfig", "amdclang++"].includes(tool.name)) return "C++(GPU)";
  return tool.category;
}
