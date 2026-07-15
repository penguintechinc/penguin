# Windows Installation & Deployment

## Penguin (CLI) and Penguind (Daemon)

### Primary: MSI installer (recommended)

The MSI is the primary Windows install path. It installs both `penguind.exe` and
`penguin.exe` into `C:\Program Files\Penguin\` and registers `penguind` as an
auto-start Windows service (Penguin Daemon, LocalSystem) via the Windows Installer.

Download `penguind.msi` from [GitHub Releases](https://github.com/penguintechinc/penguin/releases), then:

```powershell
# Interactive (double-click penguind.msi) — or silent:
msiexec /i penguind.msi /qn

# Uninstall (stops + removes the penguind service, removes files):
msiexec /x penguind.msi /qn
```

The MSI is built by the `Release Windows MSI` GitHub Actions workflow
(`.github/workflows/release-windows-msi.yml`) using WiX v5 on `windows-latest`.
The WiX source lives at `packaging/windows/penguind.wxs`.

> Note: WiX v5's MSI Bind backend requires the Windows-only `msi.dll`, so the MSI
> can only be produced on Windows (the CI runner), not on Linux/macOS.

> The published MSI is currently **unsigned** — see the signing TODO below.

### Alternative: ZIP + manual service registration

Windows builds are also distributed as **ZIP archives** containing the executables,
built by goreleaser and published to GitHub Releases.

1. Download `penguin_<version>_windows_amd64.zip` or `penguin_<version>_windows_arm64.zip` from [GitHub Releases](https://github.com/penguintechinc/penguin/releases)
2. Extract the ZIP file to your desired location (e.g., `C:\Program Files\Penguin\`)
3. Add the directory to your system `PATH` environment variable
4. Register the service manually with `penguind service install` (see below)

### Penguind as a Service (ZIP path)

Penguind supports Windows service registration via the [kardianos/service](https://github.com/kardianos/service) package, built into the daemon. To install penguind as a Windows service:

```powershell
penguind service install
penguind service start
penguind service status
penguind service stop
penguind service uninstall
```

**Supported commands:**
- `penguind service install` — Register penguind as a Windows Service (auto-start enabled)
- `penguind service uninstall` — Remove the Windows Service
- `penguind service start` — Start the service
- `penguind service stop` — Stop the service
- `penguind service status` — Check service status

**Service details:**
- Service name: `penguind`
- Startup type: Automatic
- Account: Local System

### PowerShell Example

```powershell
# Install as service
C:\Program Files\Penguin\penguind.exe service install

# Verify service is running
Get-Service -Name penguind

# View recent logs (Event Viewer)
wevtutil qe System /q:"*[System[Provider[@Name='penguind']]]" /c:10 /f:text
```

---

## Penguin Tray

Penguin Tray is built separately from goreleaser due to cgo requirements (OS-native systray integration). Tray binaries are published to GitHub Releases as part of the release workflow.

### Installation

1. Download `penguin-tray_<version>_windows_amd64.exe` or `penguin-tray_<version>_windows_arm64.exe` from [GitHub Releases](https://github.com/penguintechinc/penguin/releases)
2. Place the executable anywhere in your `PATH` or run directly
3. Tray will integrate with Windows system tray on startup

### Packaging Notes

- Tray binaries are **unsigned** (code signing deferred; see TODO below)
- No MSI installer currently provided — direct `.exe` execution or zip distribution is the supported path
- To add tray to Windows Startup:
  1. Press `Win + R`, type `shell:startup`
  2. Create a shortcut to `penguin-tray.exe`
  3. Tray will launch automatically on login

### TODO: Future Enhancements

- [ ] Code sign the `penguind.msi` with a PenguinTech Authenticode certificate (`signtool sign /fd SHA256 ...`); see the signing TODO in `.github/workflows/release-windows-msi.yml` and `packaging/windows/penguind.wxs`
- [ ] Code sign tray binaries with PenguinTech certificate (requires infrastructure setup)
- [ ] Create WiX-based `.msi` installer for tray (optional; `.exe` + service registration sufficient for now)
- [ ] Add Windows service wrapper for tray (currently manual startup or shell shortcut)

---

## Security & Trust

### Signature Verification

All published artifacts include two signatures for integrity verification:

1. **minisign** (for self-updater): `penguin_<version>_windows_amd64.zip.minisig`
2. **cosign** (for Sigstore provenance): `checksums.txt.sig` and `checksums.txt.pem`

To verify a download:

```powershell
# Via minisign (requires minisign installed)
minisign -Vm .\penguin_<version>_windows_amd64.zip -p <public_key>

# Via cosign (requires cosign installed)
cosign verify-blob --signature checksums.txt.sig --certificate checksums.txt.pem --certificate-identity-regexp ".*" --certificate-oidc-issuer https://token.actions.githubusercontent.com .\checksums.txt
```

---

## Troubleshooting

**Service fails to start:**
- Check Event Viewer (`eventvwr.msc`) → Windows Logs → Application
- Verify `penguind.exe` is in `PATH` or run install from the full path
- Ensure running PowerShell as Administrator

**Tray does not appear in system tray:**
- Verify tray binary is compatible with your Windows version (Windows 10+)
- Check `%APPDATA%\PenguinTech\penguind.log` for errors
- Try running tray manually: `penguin-tray.exe` (should launch immediately)

**Cannot add to startup:**
- Verify the file path in the shortcut is correct
- Ensure file has execute permissions
- Check Windows Startup folder: `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`
