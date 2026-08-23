[Français](README.md) · **English**

# Ruche

A multi-account Minecraft launcher for Windows, written in Rust. It opens
several clients side by side **without bringing the machine to its knees**: one
instance starts at a time, and nothing launches unless enough RAM is left.

*Ruche* is French for beehive — many workers in a single frame, a colony that
regulates itself. That is the whole idea.

It downloads no versions of its own: it reuses the official installation
(`%APPDATA%\.minecraft`), including vanilla, OptiFine, Fabric and Forge — the
`inheritsFrom` chain is resolved.

![Ruche](docs/capture.png)

## Install

A single file, nothing to set up:

1. grab `ruche.exe` from the [releases](../../releases);
2. run it. Configuration lands in `%APPDATA%\Ruche\`.

From source:

```bash
cargo build --release
```

## What keeps the machine alive

| Guard rail | Effect |
|---|---|
| RAM budget | Before every launch: `free RAM − (Xmx + overhead) ≥ reserve`. Otherwise the instance waits in the queue, then gives up with a clear message. |
| Instance cap | Extra instances wait until a slot frees up. |
| Staggered start | The queue starts **one instance at a time** and only moves on once the previous one has opened its window (detected through `EnumWindows`) or hit the timeout. |
| Modest Xmx, `-XX:-AlwaysPreTouch` | The JVM does not commit its whole heap upfront; G1 with short pauses. |
| Low priority | Clients run at `belownormal` (or `idle`), so the desktop stays responsive. |
| CPU affinity | Each instance is pinned to its own group of cores. |
| Low graphics defaults | Fresh instances get an `options.txt` with render distance 3, 60 FPS cap, sound off, `pauseOnLostFocus:false`. |

The **Calculer un réglage sûr** button re-reads free memory and suggests a number
of instances and cores. It caps at four clients: past that, VRAM gives out before
RAM does.

## Accounts

Both kinds live in the same list.

**Offline** — the UUID is derived exactly as an `online-mode=false` server does
(`MD5("OfflinePlayer:<name>")`, UUID v3). *Ajouter en lot* generates `Alt1…AltN`
in one go; *Importer du launcher* pulls names and UUIDs from
`launcher_accounts.json`.

**Premium (Microsoft)** — *device code* flow: the launcher shows a code, you
enter it on `microsoft.com/link`, and the full chain runs (Microsoft → Xbox Live
→ XSTS → Minecraft Services → profile). No password ever passes through the
launcher.

You need **your own Azure application** (free): Microsoft will not issue
Minecraft tokens to an undeclared app.

1. `portal.azure.com` → Microsoft Entra ID → App registrations → New
   registration;
2. supported account types: **personal Microsoft accounts only**, no redirect
   URI;
3. Authentication → **Allow public client flows = Yes**;
4. paste the "Application (client) ID" into the launcher — once, for every
   account.

The Minecraft session lasts 24 h and is refreshed automatically at launch for as
long as the Microsoft refresh token is valid (90 days). That token is encrypted
with **DPAPI** before being written, so `accounts.json` is unreadable from
another Windows session. On other platforms it is stored in the clear — stated
here rather than hidden.

Double-click a row to give an account its own version and heap size: alts on
1.8.9 with 1 GB while the main account runs 26.2 with 4 GB.

## Instances

Every account gets its own game directory
(`%APPDATA%\Ruche\instances\<account>`), which is what avoids file conflicts
between clients. The heavy parts stay shared with `.minecraft`: `assets` and
`libraries` are never copied, and `mods`, `resourcepacks`, `shaderpacks` and
`config` are mounted as NTFS junctions.

The **Serveur** field (`host:port`) connects straight away on launch:
`--quickPlayMultiplayer` on 1.20+, `--server/--port` before that, and the entry
is added to the instance's `servers.dat`.

Each launch writes `instances/<account>/logs/ruche-<date>.log`, whose first line
is the complete Java command — replayable as is.

## Architecture

| Module | Role |
|---|---|
| `mc::version` | reads the json files in `versions/`, merges `inheritsFrom`, evaluates OS and feature rules |
| `mc::command` | classpath, natives, argument substitution, `options.txt`, `servers.dat` |
| `mc::java` | JRE selection (official launcher runtimes, system JDKs, `PATH`) |
| `queue` | launch queue, memory guard rails, process monitoring |
| `auth` | offline UUIDs and Microsoft sign-in |
| `sys` | memory, working set, window detection, affinity, DPAPI |
| `app` | egui interface |

One detail that is expensive to get wrong: **the client jar on the classpath must
be the one inside the selected version's folder**, not the `inheritsFrom`
parent's. Installers copy it under their own name, and Forge's `ignoreList` only
covers that name — using the parent's jar makes Forge ≥ 1.17 fail with a
`ResolutionException`.

## Tests

```bash
cargo test
```

Unit tests cover version merging, rule evaluation, maven paths, offline UUIDs
(against reference values), the `servers.dat` NBT layout, DPAPI round-trips, and
the refusal to launch without memory.

Two integration tests **actually start the game** and need a Minecraft
installation, so they are ignored by default:

```bash
cargo test --test real_launch -- --ignored --nocapture
```

Verified on the development machine: 1.7.10, 1.8 / 1.8.8 / 1.8.9, 1.12.2,
1.20.1, 1.20.1-Forge 47.4.13, 1.20.2-Forge 48.1.0, 1.21.8, 1.21.11, OptiFine,
Fabric 0.18/0.19, 26.1.2, 26.2 — 24 profiles produce a complete command line,
and 1.8.9, Forge 1.20.1 and 26.2 reach the game screen.

## Known limits

- The interface is in French for now.
- Windows is the target: window detection, affinity and DPAPI are native there.
  The rest compiles elsewhere in degraded mode.
- The launcher does not download versions: one that was never launched from the
  official launcher has no client jar, and it says so.
- Missing libraries are fetched on the fly when the json provides a URL;
  otherwise the launch stops with the list of files.

## License

MIT — see [LICENSE](LICENSE).
