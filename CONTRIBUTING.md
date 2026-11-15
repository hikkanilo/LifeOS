# Contributing to LifeOS

Thanks for helping improve LifeOS. Keep the app local-first, clear about what it records, and compatible with existing user databases.

## Before you start

- Check open issues and describe the change you plan to make.
- For a bug report, include Windows version, LifeOS version, exact steps and expected/actual behavior.
- Never attach `data/`, clipboard images, browser history, screenshots containing private windows, logs with personal content, or database backups.
- Do not add telemetry, network transmission of user data, or clipboard capture enabled by default.

## Development

Install Node.js 22+, Rust stable 1.90+, Visual Studio C++ Build Tools and WebView2 Runtime. Then run:

```powershell
npm ci
npm run tauri -- dev
```

Before opening a pull request, run:

```powershell
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --all
```

## Data and migrations

- Use additive SQLite migrations. Never delete or reset existing user rows as part of an upgrade.
- Keep clipboard images as local files with metadata in SQLite, enforce size limits, and test corrupt/oversized input handling.
- Use aggregate keyboard and mouse counts only. Do not capture key values or typed text.
- Keep Firefox tracking limited to visible active window titles and skip private windows.
- Avoid hardcoded user paths, machine names, sample personal data and secrets.

## Pull requests

- Keep changes focused and explain behavior, privacy impact and migration behavior.
- Include screenshots for meaningful UI changes, using synthetic or non-sensitive content.
- Update both language sections in `README.md` and add a note to `CHANGELOG.md`.
- Do not commit `data/`, `backups/`, `logs/`, `node_modules/`, `dist/`, installer output or Rust `target/` artifacts.

## License

By contributing, you agree that your contribution is made available under the project’s MIT License.
