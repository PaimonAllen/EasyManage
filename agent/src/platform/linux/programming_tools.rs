//! Fixed-path programming tool inventory; binaries are never executed.
use super::ai_tools::executable_paths;
use easymanage_protocol::{ProgrammingToolStatus, SoftwarePackage};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

const TOOLS: &[(&str, &str, &str)] = &[
    ("rustc", "Rust", "rustc"),
    ("cargo", "Rust", "cargo"),
    ("rustup", "Rust", "rustup"),
    ("gcc", "C/C++", "gcc"),
    ("g++", "C/C++", "g++"),
    ("clang", "C/C++", "clang"),
    ("clang++", "C/C++", "clang"),
    ("cmake", "C/C++", "cmake"),
    ("make", "C/C++", "make"),
    ("nvidia-smi", "GPU", "nvidia-utils"),
    ("rocm-smi", "GPU", "rocm-smi"),
    ("rocminfo", "GPU", "rocminfo"),
    ("clinfo", "GPU", "clinfo"),
    ("matlab", "MATLAB", "matlab"),
    ("mex", "MATLAB", "matlab"),
    ("mcc", "MATLAB", "matlab"),
    ("nvcc", "C++(GPU)", "nvidia-cuda-toolkit"),
    ("cuda-gdb", "C++(GPU)", "nvidia-cuda-gdb"),
    ("compute-sanitizer", "C++(GPU)", "nvidia-cuda-toolkit"),
    ("hipcc", "C++(GPU)", "hipcc"),
    ("hipconfig", "C++(GPU)", "hipcc"),
    ("amdclang++", "C++(GPU)", "rocm-llvm"),
    ("python3", "Python", "python3"),
    ("pip3", "Python", "python3-pip"),
    ("uv", "Python", "uv"),
    ("conda", "Python", "conda"),
    ("node", "JavaScript", "nodejs"),
    ("npm", "JavaScript", "npm"),
    ("pnpm", "JavaScript", "pnpm"),
    ("corepack", "JavaScript", "corepack"),
    ("git", "通用工具", "git"),
    ("git-lfs", "通用工具", "git-lfs"),
    ("go", "通用工具", "golang-go"),
    ("java", "通用工具", "default-jre"),
    ("javac", "通用工具", "default-jdk"),
];

fn toolkit_release(name: &str) -> bool {
    ["cuda-", "rocm-"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|version| {
            version.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && version
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || b".-".contains(&byte))
        })
    })
}

fn matlab_release(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 6
        && bytes[0] == b'R'
        && bytes[1..5].iter().all(u8::is_ascii_digit)
        && matches!(bytes[5], b'a' | b'b')
}

// Scan only a bounded, single level of known installation roots, once per inventory.
// Never recursively traverse user data or execute the files being inventoried.
fn release_bins(root: &Path, accepts: fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut releases: Vec<_> = entries
        .take(128)
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_str().is_some_and(accepts))
        .map(|entry| entry.path())
        .collect();
    releases.sort();
    releases
        .into_iter()
        .take(16)
        .map(|path| path.join("bin"))
        .collect()
}

fn extra_bins() -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut gpu = ["/usr/local/cuda/bin", "/opt/rocm/bin", "/opt/rocm/hip/bin"]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    for root in ["/usr/local", "/opt"] {
        let bins = release_bins(Path::new(root), toolkit_release);
        // Older HIP installations place wrappers below hip/bin.
        gpu.extend(bins.iter().map(|bin| bin.parent().unwrap().join("hip/bin")));
        gpu.extend(bins);
    }
    let mut matlab = Vec::new();
    for root in [
        "/usr/local/MATLAB",
        "/opt/MATLAB",
        "/usr/local/matlab",
        "/opt/matlab",
    ] {
        matlab.push(Path::new(root).join("bin"));
        matlab.extend(release_bins(Path::new(root), matlab_release));
    }
    (gpu, matlab)
}

fn extend_executables(paths: &mut Vec<String>, name: &str, directories: &[PathBuf]) {
    for directory in directories {
        let path = directory.join(name);
        if fs::metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            paths.push(path.to_string_lossy().into_owned());
        }
    }
    paths.sort();
    paths.dedup();
}

pub fn inventory(home: Option<&str>, packages: &[SoftwarePackage]) -> Vec<ProgrammingToolStatus> {
    let (gpu_bins, matlab_bins) = extra_bins();
    TOOLS
        .iter()
        .map(|(name, category, package)| {
            let mut executable_paths = executable_paths(name, home);
            match *category {
                "GPU" | "C++(GPU)" => extend_executables(&mut executable_paths, name, &gpu_bins),
                "MATLAB" => extend_executables(&mut executable_paths, name, &matlab_bins),
                _ => {}
            }
            let package_version = (!executable_paths.is_empty())
                .then(|| {
                    packages
                        .iter()
                        .find(|entry| entry.name == *package)
                        .map(|entry| entry.version.clone())
                })
                .flatten();
            ProgrammingToolStatus {
                name: (*name).to_owned(),
                category: (*category).to_owned(),
                executable_paths,
                package_version,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use easymanage_protocol::SoftwarePackage;

    #[test]
    fn reports_only_package_versions_for_detected_tools() {
        let tools = inventory(
            None,
            &[SoftwarePackage {
                name: "not-a-tool".into(),
                version: "1".into(),
            }],
        );
        assert!(tools.iter().any(|tool| tool.name == "rustc"));
        assert!(tools.iter().all(|tool| tool.package_version.is_none()));
    }

    #[test]
    fn includes_gpu_matlab_and_gpu_cpp_checklists() {
        let tools = inventory(None, &[]);
        for (name, category) in [
            ("nvidia-smi", "GPU"),
            ("matlab", "MATLAB"),
            ("mex", "MATLAB"),
            ("nvcc", "C++(GPU)"),
            ("hipcc", "C++(GPU)"),
        ] {
            assert!(
                tools
                    .iter()
                    .any(|tool| tool.name == name && tool.category == category)
            );
        }
        let names: std::collections::HashSet<_> = tools.iter().map(|tool| &tool.name).collect();
        assert_eq!(names.len(), tools.len());
    }

    #[test]
    fn release_names_are_strictly_bounded() {
        for name in ["cuda-12.8", "rocm-6.2.1"] {
            assert!(toolkit_release(name));
        }
        for name in [
            "cuda-",
            "cuda-other",
            "rocm-../other",
            "other-1",
            "cuda-1/bin",
        ] {
            assert!(!toolkit_release(name));
        }
        for name in ["R2025b", "R2026a"] {
            assert!(matlab_release(name));
        }
        for name in ["R2025c", "R25a", "R2025b/other", "other"] {
            assert!(!matlab_release(name));
        }
    }

    #[test]
    fn discovers_release_binaries_without_executing_them() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let fixture = Fixture(
            std::env::temp_dir().join(format!("programming-tools-{}", uuid::Uuid::new_v4())),
        );
        let bin = fixture.0.join("R2025b/bin");
        fs::create_dir_all(&bin).unwrap();
        let sentinel = fixture.0.join("must-not-exist");
        let executable = bin.join("matlab");
        fs::write(
            &executable,
            format!("#!/bin/sh\ntouch '{}'\n", sentinel.display()),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(bin.join("mex"), "not executable").unwrap();
        let directories = release_bins(&fixture.0, matlab_release);
        assert_eq!(directories, vec![bin]);
        let mut paths = Vec::new();
        extend_executables(&mut paths, "matlab", &directories);
        extend_executables(&mut paths, "matlab", &directories);
        assert_eq!(paths, vec![executable.to_string_lossy().into_owned()]);
        let mut missing = Vec::new();
        extend_executables(&mut missing, "mex", &directories);
        assert!(missing.is_empty());
        assert!(!sentinel.exists());
    }
}
