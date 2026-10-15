try {
  const theme = localStorage.getItem("easymanage-theme") ?? localStorage.getItem("vitepress-theme-appearance");
  document.documentElement.dataset.theme = theme === "light" ? "light" : "dark";
} catch {
  document.documentElement.dataset.theme = "dark";
}
