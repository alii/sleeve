# Building Sleeve on Windows

The README covers macOS. This is what a Windows machine needs before `cargo build` works, and why each one is needed.

## Quick list

| What | Why | Install |
| --- | --- | --- |
| Rust (MSVC toolchain) | Compiles the code | [rustup.rs](https://rustup.rs) |
| Visual Studio Build Tools with **Desktop development with C++** | Provides `link.exe`, the MSVC compiler and the Windows SDK | See [Linker](#1-the-msvc-linker-linkexe) |
| Strawberry Perl | OpenSSL's build scripts are written in Perl | `winget install StrawberryPerl.StrawberryPerl` |
| Access to the GitHub git dependencies | Some crates come straight from GitHub | See [Git dependencies](#3-git-dependencies) |

After installing anything, **open a new terminal** before building. Installers change `PATH`, and terminals that were already open don't see the change.

## 1. The MSVC linker (`link.exe`)

**Error:**

```
error: linker `link.exe` not found
note: the msvc targets depend on the msvc linker but `link.exe` was not found
```

**Why:** On Windows, Rust's default toolchain is `x86_64-pc-windows-msvc`. `rustc` compiles code into object files, then Microsoft's linker, `link.exe`, combines them with Windows system libraries into an `.exe`. Rust doesn't ship `link.exe`; it comes with Visual Studio's C++ tools. It fails almost straight away because many crates have a `build.rs` script that has to be compiled and linked before anything else.

**Fix:** Install Build Tools for Visual Studio 2022 **with the C++ workload**.

The catch: `winget install Microsoft.VisualStudio.2022.BuildTools` on its own installs Build Tools with **no workloads**, which doesn't help. And if Build Tools is already installed, running `winget install ... --override "--add ..."` doesn't add anything either, because winget only tries to upgrade. In that case, modify the existing install instead:

- **GUI:** open *Visual Studio Installer*, click **Modify** on Build Tools 2022, tick **Desktop development with C++** (keep the MSVC v143 tools and a Windows 11 SDK ticked), then click **Modify**.
- **Terminal (PowerShell):**

  ```powershell
  & "C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe" modify --installPath "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools" --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --passive
  ```

On a fresh machine, this one command installs Build Tools with the workload already included:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

**Check what's installed:** this only reads, and prints the install path if the C++ tools are present:

```powershell
& "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

You don't need the "Developer Command Prompt". Rust finds the tools by itself.

## 2. Perl (for OpenSSL)

**Error:** the `openssl-src` crate fails, complaining that Perl can't be found.

**Why:** `Cargo.toml` has:

```toml
rusqlite = { version = "0.40", features = ["bundled-sqlcipher-vendored-openssl"] }
```

rekordbox's `master.db` is encrypted with SQLCipher. This feature builds SQLCipher from source, and also builds **OpenSSL** from source (that's the "vendored" part) instead of using one installed on the system. OpenSSL's build system (`Configure` and the makefile generators) is written in Perl. macOS comes with Perl; Windows doesn't.

**Fix:**

```powershell
winget install StrawberryPerl.StrawberryPerl
```

Open a new terminal and check it with `perl -v`.

Don't use the Perl that comes with Git Bash (`/usr/bin/perl`). PowerShell can't see it, and it's missing modules that OpenSSL's build needs.

**Expect it to be slow:** OpenSSL is a large C codebase. The first build sits on `openssl-src` for several minutes. Later builds use Cargo's cache.

## 3. Git dependencies

`rune`, `rune-kit`, `rune-theme-opencode` and all the `rbl-*` crates come straight from GitHub repos pinned to exact commits, not from crates.io. That means **`cargo build` runs git fetches itself**, even though you never typed a git command.

**Error you might see:**

```
revision d0ff75b378ec85be891d9f3067ada1e82bad4cae not found
failed to authenticate when downloading repository: git@github.com:chrisle/rbxport
```

**What it means:** Cargo's built-in git client tried SSH and had no credentials. That usually happens when a global git config rewrites `https://github.com/` URLs to `git@github.com:`.

**Fix:** tell Cargo to use your normal `git` program, so it uses your usual SSH agent or credential helper, hardware keys included. This only applies to the one command:

```powershell
$env:CARGO_NET_GIT_FETCH_WITH_CLI = "true"; cargo build
```

Once the dependencies are downloaded, Cargo caches them in `~/.cargo/git`, and later builds don't fetch again unless `Cargo.lock` changes.

## Build and run

```powershell
cargo build              # debug build: quicker to compile, slower app
cargo run --release      # optimised build, then runs the app
cargo test               # run the tests
```

## Troubleshooting

**Hundreds of `warning LNK4099: PDB 'ossl_static.pdb' was not found`:** harmless. The linker can't find debug info for OpenSSL's compiled pieces and links them without it. They're warnings, not errors.

**`wgpu-hal` fails with *expected `ID3D12Device`, found a different `ID3D12Device`* / *multiple different versions of crate `windows`*:** `gpu-allocator` has ended up on a different version of the `windows` crate than `wgpu-hal` (usually after `cargo update`). In `Cargo.lock`, the `gpu-allocator` entry's dependencies should list `"windows 0.62.2"`, not `"windows 0.57.0"`. Check with:

```powershell
cargo tree --offline --locked -p gpu-allocator --target x86_64-pc-windows-msvc -e normal --depth 1   # should show windows v0.62.2
```

**`cargo test` fails `export::tests::pulling_the_stick_out_fails_cleanly_and_the_next_export_finishes`:** expected on Windows. The test fakes unplugging a USB stick by renaming its folder in the middle of a copy, which Windows doesn't allow while files inside are open.
