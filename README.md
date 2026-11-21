# SoftVault 🔐

A zero-knowledge desktop vault for **passwords and secure notes**, written in Rust.
Its **light** and **dark** themes follow the Bitwarden web-vault look: navy sidebar, blue primary color, Inter font, pill buttons, a table of items and item dialogs. It stores everything in **SQLite**, and the CI builds a **Windows `.exe`**.

SoftVault is an independent project. It is not affiliated with Bitwarden and uses its own name and logo.

## Features

- **Zero-knowledge encryption.** Your master password is never stored. Everything is encrypted before it reaches the disk.
- **Passwords and secure notes**, with favorites, search, copy buttons, a strength meter and a password generator.
- **Rich secure notes.** Each note has a format, and you can switch between **Edit**, **Split** (live preview) and **Preview**:
  - **Markdown**: headings, bold/italic/strikethrough, lists, task lists (☑/☐), quotes, tables, links, and fenced code blocks with syntax highlighting. A toolbar inserts formatting at the cursor.
  - **Code**: syntax highlighting for many languages (rust, py, js, ts, sql, sh, yaml, toml, go, c, cpp, cs…) with line numbers.
  - **JSON**: a validity check, a collapsible tree view, and a "Format JSON" button that keeps your key order.
  - **CSV**: shown as a table. The delimiter (comma, semicolon, tab or pipe) is detected automatically and the first row is used as the header.
  - **Plain text**.
  - **Full-screen reader** (**⛶ Full screen** in the note editor, or **⋮ → Open full screen** in the list). It fills the window with a centered page and offers:
    - a clickable table of contents for Markdown headings
    - text zoom (A− / A+)
    - **F11** for true Windows full screen
    - copy
    - **Esc** to close
  - Previews never load anything from the network. Markdown images show as placeholders, HTML is shown as text, and only `http`, `https` and `mailto` links can be opened.
- **Optional 2FA with your phone.** Use any TOTP authenticator: Google Authenticator, Microsoft Authenticator, Authy, 2FAS or Aegis. You enroll by scanning a QR code.
- **Password hint by Gmail.** The hint never appears on screen. It can only be emailed to a Gmail address you have **verified** with a 6-digit code.
- **Export:**
  - **encrypted** `.svx` (Argon2id + XChaCha20-Poly1305, protected by a password you choose)
  - **unencrypted** `.json`, which asks you to confirm before it saves
- **Import** from either format.
- **Auto-lock** after a period of inactivity, and **clipboard auto-clear** after you copy.
- **Drag & drop encryption (Secure files).**
  - **Drag in:** drop files or folders anywhere on the window and they're encrypted into the vault with your vault key (XChaCha20-Poly1305, with the name and content encrypted separately). Text files (`.md .txt .json .csv` and code) become **secure notes** in the matching format. Everything else (PDFs, images, keys, archives, up to 200 MB each) is stored under **Secure files**.
  - **Drag out (Windows):** drag a file card, or a note in the vault list, out to Explorer or the desktop to get a **decrypted** copy. SoftVault writes it to a private temp folder for Windows' native drag, then overwrites and deletes that temp copy after 2 minutes (and immediately on lock or exit). **Save decrypted…** does the same through a save dialog.
  - **Move mode (optional):** securely delete the original files after they've been encrypted. They are overwritten with zeros first; SSDs may still keep old blocks.
  - Files are included in exports and backups, and survive lock and unlock.
- **Safe, animated delete.** Deleting takes two clicks on an animated recycle bin: the lid lifts on hover, and the first click arms it (it turns red and wiggles). The second click drops the item in and closes the lid. An **Undo** toast then lets you restore the item for 10 seconds.
- **Motion:** pages and dialogs fade in, toasts slide in, and the sidebar and table rows fade on hover.
- **Vault health:** a health score and lists of weak, reused and old passwords, each with a **Fix** button.
- **Productive UI:** Lucide icons, sortable columns, quick actions when you hover a row (copy username or password, open website), a right-click menu, tabbed settings, an auto-lock countdown, and keyboard shortcuts (Ctrl+F, Ctrl+N, Ctrl+Shift+N, Ctrl+L, Ctrl+S, Esc).
- Per-login **2FA codes** (optional): store a site's TOTP secret and get live codes.

## Security model

| Layer | Choice |
|---|---|
| Key derivation | **Argon2id** (RFC 9106): 256 MiB memory, 3 passes, 4 lanes, 256-bit random salt |
| Encryption | **XChaCha20-Poly1305** AEAD: 256-bit key, fresh 192-bit random nonce per record |
| Key hierarchy | master password → Argon2id → KEK → unwraps a random 256-bit **vault key** → encrypts every record |
| Record binding | each ciphertext is bound to its row ID through AAD, so swapping or replaying rows is detected |
| Metadata | titles, usernames, URLs, timestamps and item types are **all encrypted**. SQLite only stores random IDs and ciphertext |
| Memory | decrypted data and keys are wiped with `zeroize` when the vault locks |
| Brute force | each guess costs a full 256 MiB Argon2id run, and the app adds an increasing lockout after 3 failures |
| Hint + Gmail app password | protected with **Windows DPAPI**, which ties them to your Windows user account |

Some honest notes:
- **2FA** is enforced by the app every time you unlock. However, the vault file's encryption depends **only on your master password**. If someone steals `vault.db`, the TOTP code does not stop them from guessing passwords offline. Use a strong master password (the app requires 12+ characters).
- The **password hint** must be readable while the vault is locked, so it can't be zero-knowledge. That's why it is DPAPI-protected, never shown on screen, and only emailed to your verified Gmail. Don't put your password in the hint.
- There is **no recovery** if you forget the master password. That is how zero-knowledge works.

## Gmail setup (for hint delivery)

1. Turn on **2-Step Verification** for your Google account.
2. Go to **myaccount.google.com → Security → 2-Step Verification → App passwords** and create one. It's 16 characters.
3. In SoftVault, open **Settings → Password hint & recovery Gmail**, enter your Gmail address and the app password, click **Send verification code**, then enter the code from the email.
4. Set your hint. On the lock screen, **Forgot your password? → Email my hint** sends it through `smtp.gmail.com:465` (TLS), from your Gmail to your Gmail.

## Data location (portable)

The vault is stored as **`vault.db` in the same folder as `SoftVault.exe`**, so backing up that folder backs up everything. The database is fully encrypted.

- If the exe's folder is read-only (for example under `Program Files`), SoftVault falls back to `%APPDATA%\SoftVault\vault.db`.
- **Settings → Backup & storage** shows the exact path, has an **Open folder** button, and can make a consistent **Back up now** copy while the app is running.
- The Gmail recovery settings and the hint are protected with Windows DPAPI, so they only work for the same Windows account. On another PC your vault still opens with the master password; you just set up recovery again.

## Build

### Using GitHub Actions (Windows `.exe`)
The workflow is in `.github/workflows/windows-build.yml`.

1. Push this folder to a GitHub repository.
2. Each push or pull request to `main`/`master` runs the tests and builds `SoftVault.exe` on `windows-latest` with a static CRT, so no VC++ redistributable is needed.
3. Download the `.exe` from **Actions → the run → Artifacts → SoftVault-windows-x64**.
4. Push a tag such as `v1.0.0` (`git tag v1.0.0 && git push --tags`) to also publish a **GitHub Release** with the zipped `.exe` and its SHA-256 checksum.
5. You can also start a build by hand with **Run workflow** (`workflow_dispatch`).

### Locally on Windows
```powershell
# Rust from https://rustup.rs (MSVC toolchain)
cargo test --release
cargo build --release
.\target\release\softvault.exe
```

## Project layout

```
assets/fonts/  Inter (SIL OFL) and a Lucide icon subset (ISC); licenses included
src/
  main.rs       window setup and icon
  app.rs        all screens: setup / unlock / 2FA / vault / settings
  widgets.rs    widgets: pill buttons, sections, sidebar, chips, table rows, QR code
  theme.rs      light/dark palettes and Inter font setup
  crypto.rs     Argon2id + XChaCha20-Poly1305, key wrapping, encrypted export
  vault.rs      vault logic and in-memory state (zeroized on lock)
  db.rs         SQLite storage (ciphertext only)
  render.rs     note rendering: Markdown, code highlighting, JSON tree, CSV table
  transfer.rs   drag & drop in/out, temp-copy shredding, native Windows drag source
  totp.rs       RFC 6238 TOTP (tested against RFC 4226 vectors)
  mailer.rs     Gmail SMTP (rustls)
  protect.rs    Windows DPAPI wrapper
  generator.rs  CSPRNG password generator and strength estimate
  icons.rs      Lucide icon codepoints
```
