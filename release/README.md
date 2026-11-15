# GitHub Release checklist

Attach the signed or checksum-verified installer generated at:

`src-tauri/target/release/bundle/nsis/LifeOS_*_x64-setup.exe`

For each version:

1. Update the versions in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Update `CHANGELOG.md` and prepare bilingual release notes.
3. Run `Setup-LifeOS.ps1` or the Windows Release workflow.
4. Attach the NSIS installer and publish its SHA-256 checksum in the release notes.
5. State Windows and WebView2 requirements, known sensor limitations, and any database migration.
6. Keep user databases, clipboard files, logs, backups and portable `data/` folders out of release assets.

The included workflow builds and uploads an artifact to GitHub Actions only. It does not publish a GitHub Release.
