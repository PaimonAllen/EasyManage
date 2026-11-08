# Proxy analysis dependency

The platform runs **Mihomo as a separate executable**, on demand. The source is
checked out under the ignored `third_party/mihomo/`; the upstream license remains
GPL-3.0. Repository hygiene deliberately keeps third-party source and downloaded
binaries out of EasyManage Git history. `mihomo.json` records the source commit,
release and the SHA-256 of the archive retrieved over HTTPS on the stated date.
This checksum is a pinned local download checksum, not an upstream signature.

Reproduce the source checkout from the repository root:

```sh
git clone --depth 1 --branch v1.19.33 https://github.com/MetaCubeX/mihomo.git third_party/mihomo
git -C third_party/mihomo rev-parse HEAD
```

Expected commit: `3f27ef39f8c96bbfbc8b0b9bb10f5c9ef60149d8`.
Preserve its `LICENSE` and the corresponding source when redistributing Mihomo.
Upstream API documentation: <https://wiki.metacubex.one/api/>.

For Linux x86_64, run `python3 deploy/third-party/install-proxy-core.py` to inspect,
then add `--apply` to download and install the pinned core as
`~/.easymanage/bin/mihomo-probe` (0700). Existing files are refused rather than
overwritten. The installer does not start a service, load user configurations,
change firewall/routes or touch an existing Clash/Mihomo process.
For offline/restricted deployment, `--archive /path/to/downloaded.gz --apply`
uses an existing archive with the same mandatory pinned checksum.
Other deployments can provide an absolute `EASYMANAGE_PROXY_CORE_BINARY` pointing
to a trusted, executable regular file owned by root or the platform user without
group/other write permission. Do not use a group-writable shared executable.

The runtime uses a private Unix controller socket, a random controller secret,
no TUN, no LAN listener and no imported rules or subscription downloads. IP
scoring alone opens an authenticated random loopback proxy port. There is at
most one availability core and one manually requested scoring core. The cores
exit after their batch, with hard watchdog deadlines of 175/660 seconds,
`GOMAXPROCS=1` and a **soft** Go memory target of 48 MiB each. This is not a hard
memory limit, and target-node traffic and TLS handshakes still have a real cost.
Generated core configurations temporarily use private 0600 files in 0700
workspaces. Normal exit removes them; the worker periodically removes abandoned
workspaces older than 15 minutes, after the core watchdog deadline. These
temporary files must not be included in backups or Git. Application network
counters record probe attempts and API response bodies; they cannot measure all
bytes inside the separate proxy core, TLS or network retransmissions.

All imports join a single sequential availability queue by default. Each profile
is due three minutes after its last check started; busy queues defer checks
rather than running parallel cores or creating missing observations. Pausing one
profile retains its history and leaves other profiles running. Scoring never runs
on a schedule. Database ciphertext requires the matching private
`~/.easymanage/config/proxy-lab/master.key`; back up that key separately with
0600 permissions. Losing it does not trigger automatic key replacement.
