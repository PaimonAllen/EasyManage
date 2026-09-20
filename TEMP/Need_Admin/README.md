# Administrator-required setup

The current Ubuntu host already has the required C/C++ compiler toolchain, CMake, Ninja,
OpenSSL and SQLite development packages, Protobuf, PostgreSQL client access, Node.js, pnpm,
and a running PostgreSQL service.

Run the scripts independently according to the capability needed:

```bash
./TEMP/Need_Admin/install_system_build_dependencies_ubuntu.sh
./TEMP/Need_Admin/install_global_rust_toolchain_ubuntu.sh
```

- `install_system_build_dependencies_ubuntu.sh` makes the compiler toolchain, CMake/Ninja,
  SQLite CLI and development library, OpenSSL development library, Protobuf compiler,
  PostgreSQL client, and script tooling available system-wide. Apt makes repeated runs safe.
- `install_global_rust_toolchain_ubuntu.sh` installs the official Rust 1.98.1 standalone
  toolchain under `/usr/local`, making it available to all users. Ubuntu 22.04's Rust 1.75
  package is not used because this workspace uses Rust 2024 edition.

The current user also has a per-user rustup installation for immediate development. It is
left intact and takes precedence in that user's shell.
