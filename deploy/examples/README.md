# Public deployment examples

`fleet.example.json` contains documentation-only addresses and dummy node IDs. Copy it to ignored `deploy/local/fleet.json`, configure verified SSH targets and existing node identities, then remove the `example` marker. The default never executes the public example.

Do not commit production credentials, host identities, private keys or runtime state here. Current fleet targets are Linux only; Windows/macOS preparation folders are not supported deployment targets.
