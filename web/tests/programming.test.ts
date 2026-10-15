// Run only in the documented isolated virtual environment, not on production.
import assert from "node:assert/strict";
import { test } from "node:test";
import { PROGRAMMING_CATEGORIES, PROGRAMMING_CATEGORY_HINTS, displayProgrammingCategory } from "../src/lib/programming.ts";

test("GPU, MATLAB and GPU C++ each have a separate filter and an honest scope hint", () => {
  for (const category of ["GPU", "MATLAB", "C++(GPU)"]) {
    assert.ok(PROGRAMMING_CATEGORIES.includes(category));
    assert.ok(PROGRAMMING_CATEGORY_HINTS[category]);
  }
  assert.equal(new Set(PROGRAMMING_CATEGORIES).size, PROGRAMMING_CATEGORIES.length);
});

test("known tools keep their exact category even when reported with a legacy category", () => {
  for (const [name, expected] of [["nvidia-smi", "GPU"], ["clinfo", "GPU"], ["matlab", "MATLAB"], ["mex", "MATLAB"], ["mcc", "MATLAB"], ["nvcc", "C++(GPU)"], ["hipcc", "C++(GPU)"], ["amdclang++", "C++(GPU)"], ["cmake", "构建工具"], ["go", "Go"], ["javac", "Java"], ["rustc", "Rust"]]) {
    assert.equal(displayProgrammingCategory({ name, category: name === "rustc" ? "Rust" : "通用工具", executable_paths: [], package_version: null }), expected);
  }
});
