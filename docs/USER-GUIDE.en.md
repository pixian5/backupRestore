# BackupRestore User Guide (English)

> **Applies to: v2.6.62 (2026-10-26).** This is a **living document** (like `ERROR-CODES.md`, an
> intentionally named evergreen file). It describes end-user visible behaviour; development and
> acceptance details live in the dated files under `docs/`, and the error-code lookup lives in
> [`ERROR-CODES.md`](./ERROR-CODES.md).
> Where this text and the program disagree, **the program wins** — please report the difference.
> Chinese original: [`USER-GUIDE.md`](./USER-GUIDE.md).

---

## 1. What this is

BackupRestore is a one-click Windows backup/restore tool (development test build). It does four things:

| Capability | One-line summary | Destructive? |
|---|---|---|
| Backup | Captures the selected partition into a WIM image | No (reads the source, writes only the image file) |
| Restore system to C: | Writes an image back onto the running system drive and rebuilds boot | **Yes (overwrites everything on the system drive)** |
| Restore data to non-C: | Applies an image to another partition online | **Yes (overwrites the target volume)** |
| Add second system (non-C:) | Installs a full Windows onto another partition and adds it to the boot menu | **Yes (overwrites the target partition)** |

A sixth entry, `Install PE`, builds/installs a Windows PE environment; it is a separate, simpler path.

**Support boundary:** Windows 10/11, UEFI + GPT, NTFS system partition. **Explicitly out of scope:**
incremental/differential images, network backup, Legacy BIOS (MBR), automatic BitLocker handling.
Binding a second system by drive letter on an MBR disk does not work — that is a hard boundary.

---

## 2. Prerequisites

1. **Run as administrator.** The program self-checks; without elevation it cannot read volume
   identities and therefore cannot create a task.
2. **Start from `H:\brwork\BackupRestore.exe`.** All five deployed files live in `H:\brwork`
   (`BackupRestore.exe`, `Recovery.exe`, `winpe-winpeshl.ini`, `wimlib-imagex.exe`, `libwim-15.dll`).
   Task records and logs also live under `H:\brwork` — **do not copy just the exe elsewhere and run
   it**, that gives you a different task view.
3. Keep the image (.wim) on a volume that is **not** the backup source or the restore target.

---

## 3. Using the GUI

### 3.1 Layout

Main window after start (the title bar shows the version, e.g. `BackupRestore - Rust GUI v2.6.62`):

- **Operation mode** (a row of radio entries): `Backup` / `Restore system to C:` /
  `Restore data to non-C:` / `Add second system (non-C:)` / `Install PE`
- **Top-bar buttons**: `View logs` (lists the most recently modified log files; open one in
  Notepad or open the log folder), `Backup files…` (lists the WIMs and indexes on the image
  volume; delete the oldest indexes and keep only the latest N), `Cancel pending reboot`
  (abandon a task that is waiting for a reboot), `Settings…`, `Language` (Chinese/English,
  remembered across restarts)
- **Main area** changes with the entry and usually holds: source volume dropdown, image path
  (`Browse…` to pick), compression, image name, keep latest N, the `Format target volume`
  checkbox (restore entries), `Read image` (parses the selected .wim and fills in its indexes)
- **Bottom buttons**: `Refresh environment` (re-detect Windows, recovery environment and volume
  identities — **click it first on a fresh start**), `Read image`, `Create task`,
  `Refresh task status`
- Every entry has an explanation block underneath stating what it reads, what it writes and whether
  it reboots — **read it before acting**.

### 3.2 First run: click "Refresh environment"

Click `Refresh environment` and confirm that the current Windows, the recovery environment
(WinRE / PE entry) and every volume identity are recognised. Identity is matched by
**disk + partition GUID**, not by drive letter, so changed letters do not break recognition.

### 3.3 First run: run an environment self-check (command line)

> **The GUI no longer offers an `Inspect` entry (removed on 2026-10-26 at the user's request).**
> The capability still exists on the command line via the same `prepare` path.

To validate the environment without touching any data, run this from the program directory:

```
BackupRestore.exe prepare --operation probe --source-drive C --no-reboot
```

It performs an **environment self-check only**: it validates the current Windows, the recovery
environment (WinRE / PE entry) and every volume identity (by disk + partition GUID), and writes the
task files plus recovery payload to confirm the setup is wired correctly.

- It **never backs up, restores, formats, touches boot entries, or reboots** (`--no-reboot`);
- Afterwards check the task status and the log: a successful preparation means the self-check passed,
  and you can use the backup/restore entries with confidence;
- A self-check task never turns into a "waiting for reboot" task — it does not reboot by design, so it
  never blocks your next operation.

### 3.4 Backup

1. Select `Backup`; the source defaults to the current system partition (another partition can be
   selected).
2. Enter an **absolute** image path (e.g. `H:\BRImages\my-backup.wim`); when the image already
   exists the new capture is appended as a new index.
3. Compression: `Compression` = fast (balanced, default), `No compression` = fastest but largest.
4. The image name defaults to "system version + boot time" and is editable; `Keep latest N` prunes
   old indexes automatically (0 = never prune).
5. Click `Create task` → confirmation dialog (for backup it only states which volume is read and
   which image is written) → after confirming:
   - **Data volume**: completed online inside the current Windows; success when the progress finishes;
   - **System volume**: reboots into the recovery environment, captures offline, then returns to
     Windows automatically.
6. On success the task record and logs stay under `H:\brwork` (see §7).

> **Space pre-check (runs when the task is created, writes nothing):** the free space on the
> **volume holding the image** is inspected first.
> Below "source volume used ÷ 20 (at least 1 GiB)" → the task is rejected with `BR-IMG-008` and
> **not a single byte of image data is written**;
> below the conservative estimate (`Compression` = 70% of the source usage, `No compression` = 105%)
> → **only a log warning, no rejection**.
> Why the estimate is not a gate: the WIM ratio swings between 13% and 100% depending on content
> (measured ~13% for a system drive with `Compression`, close to 100% for a data volume full of
> already-compressed media), so gating on it would reject backups that would actually fit.

### 3.5 Restore

The three restore entries differ by target; all of them **overwrite data on the target** and all ask
for confirmation:

- `Restore system to C:`: target is locked to the running system drive; `Format target volume` is
  checked by default (format, verify success, then write the image; unchecking it means an overlay
  apply and leftover files remain). It runs in the recovery environment, repairs boot and reboots
  back into Windows.
- `Restore data to non-C:`: runs online, target selectable; formatting is optional here too.
- `Add second system (non-C:)`: installs a full Windows onto another partition and adds it to the
  boot menu (dual boot).

First-class evidence of a successful restore (the program verifies each item and refuses to report
success otherwise): the target volume serial changed, the boot menu entry exists, the default boot
entry points at the restored system, and the recovery environment registration is intact.

### 3.6 If it gets stuck

- Inside the recovery environment you can press `P` to pause/resume, `B` to roll back the boot
  configuration data, and `L` / `M` / `K` to view logs (there are buttons as well as shortcuts);
  the program waits for the current step to reach a boundary before closing a window.
- Top-bar `Cancel pending reboot`: abandon a task that is "waiting for the reboot to continue"
  (all evidence is kept; only the one-shot boot entry is cleared).
- Unattended, once failures reach the threshold (default 3), the program cleans up and reboots back
  into Windows — it never leaves the machine inside the recovery environment.

---

## 4. Command line

`H:\brwork\BackupRestore.exe <command> [options]`. With no arguments it prints the full command table.

| Command | Purpose |
|---|---|
| `online --operation backup\|restore-existing\|create-secondary --source-drive <letter> [--target-drive <letter>] [--image-path <abs path>] [--index <n>] [--compress <fast\|none>] [--image-name <name>] [--keep-indexes <n>] [--format-target] [--ui-language <zh\|en>]` | Online path: data-volume backup/restore, second system. Omitting `--compress` means fast |
| `prepare --operation probe\|backup\|restore-existing\|create-secondary --source-drive <letter> [--target-drive <letter>] [--image-path <path>] [--wim-index <n>] [--boot-menu-name <name>] [--skip-format] [--fail-threshold <1-20>] [--fail-action <auto-escape\|pause>] [--ui-language <zh\|en>] …` | Preparation phase: validate, write the task, inject the recovery payload, set the one-shot boot entry |
| `resume <task-root> <task-id>` | Resume or clean up an existing task (`<task-root>` is the **workspace root**, e.g. `H:\brwork`, not the tasks directory) |
| `recover <task-root> <task-id> [--dry-run] [--efi-root <mounted EFI root>]` | Manual resume channel inside the recovery environment / PE |
| `wim-info <abs path> [--skip-hash]` | Read-only parse of image indexes and metadata |
| `status <task-root> <task-id>` / `hash <file>` / `list-volumes` / `inspect-environment` | Task status / SHA-256 of a file / list volumes / environment self-check |
| `--json` | Any command: emit the verdict as **one line of JSON** (`command/operation/success/task_id/log_path/error/error_code`) |

**For automation always use `--json`:** judge on fields (`success`, `error_code`) and never parse log
text — log text exists in several languages.

**Language:** the `--ui-language` flag wins, then the `BACKUPRESTORE_LANG` environment variable, then
`ui_language` in the settings file, then Chinese. Example: `set BACKUPRESTORE_LANG=en` makes every
command log and verdict English.

---

## 5. What happens during a recovery-environment run

Taking "Restore system to C:" from the moment you click `Create task`:

```text
Inside Windows, prepare: validate -> save a BCD snapshot -> inject a standalone WinRE payload
   -> set the one-shot boot entry -> reboot into the recovery environment
      -> Recovery.exe starts automatically
      -> apply the image offline (DISM primary; on failure it automatically falls back to wimlib once)
      -> bcdboot repairs boot + read-back verification (default entry / menu / resumeobject must
         all match before success is reported)
      -> clean up this task's temporary boot entry and payload copy
   -> reboot back into normal Windows
```

- Failure midway: retried per the configured failure policy (3 times by default); inside the recovery
  environment you can also press `P` to pause and `B` to roll back the BCD.
- "Automatic wimlib fallback" is not a fault: when DISM times out or fails, the program switches to
  the wimlib channel automatically (the log contains a fallback line) and the task still settles as
  success.
- Nothing outside the recovery environment itself is modified; task records
  (`task.json` / `status.json` / logs) are all kept in `H:\brwork\tasks\<task-id>\`.

---

## 6. Helping yourself: read the error code

Every runtime failure message carries a **stable error code**, formatted `[BR-domain-number] text`:

1. **Find the code**: `grep -a "[BR-"` in the log (PowerShell: `Select-String -Pattern '\[BR-'`), or
   read the bracketed prefix after `error:` on the command line; with `--json` just read the
   `error_code` field.
2. **Look it up** in [`ERROR-CODES.md`](./ERROR-CODES.md) (214 bilingual codes plus 3 structural ones such as `BR-IO-001`).
   **Codes are a contract and never change** — quoting the code alone is enough to locate the fault.
3. **Command-line usage errors** (wrong arguments, etc.) are **deliberately codeless** — the English
   text identifies them; only runtime failures carry codes.

Quick reference for common failures:

| Symptom / code | Meaning | What to do |
|---|---|---|
| Target too small (`BR-PE-048`) | The target volume cannot hold the expanded image | Use a larger target, or enable formatting and retry |
| Image volume too small (`BR-IMG-008`) | Pre-check before backup: free space on the image volume is below "source used ÷ 20 (at least 1 GiB)" | Use a larger image volume or free space first; **no image data was written this run** |
| File in use (log category "in use") | A program on the target holds a file | Close the program and retry; rare inside the recovery environment |
| Corrupt image (log category "corrupt image" + `BR-APL-001/004`) | The image file itself is damaged; **no** automatic fallback | Use another image; never treat a broken image as a backup |
| DISM timeout (log category "timeout") | DISM stalled past the limit | The program falls back to wimlib automatically; tune `dism_timeout_secs` in Settings |
| Sidecar mismatch (`BR-PE-047`) | `.index-N.metadata.json` next to the image does not match it | Do not hand-edit sidecars; re-back up or delete stale ones |
| wimlib missing (`BR-APL-002/003`) | `wimlib-imagex.exe` or `libwim-15.dll` is missing from the program directory | Put both files back into `H:\brwork` |
| `BR-IO-001` | A file/system call failed (text follows the OS language) | Read the path in the text: usually missing / in use / permission |

See §7 for log locations. **When reporting an issue, attach: the error code + the log covering that
time window from `H:\brwork\logs`.**

---

## 7. Settings and logs

### 7.1 Settings (`Settings…` button → writes `H:\brwork\BackupRestore.settings.ini`)

| Key | Meaning | Default |
|---|---|---|
| `failure_threshold` | How many consecutive failures inside the recovery environment trigger cleanup and a reboot back to Windows (1–20) | 3 |
| `failure_action` | Action at the threshold: `auto-escape` (clean up and reboot) / `pause` (show a dialog and wait for a human) | auto-escape |
| `failure_pause_max_wait_secs` | Total unattended wait cap for the `pause` policy (seconds, 0 = unlimited) | 0 |
| `dism_timeout_secs` | DISM wait cap (seconds, 0 = unlimited) | 0 |
| `ui_language` | UI language `zh`/`en` (also switchable from the top-bar Language dropdown; remembered) | zh |

Settings apply to tasks created **afterwards**; a running task keeps the settings it was created with.
(Maintainer note: several modules share this file, so anyone writing it must merge — replace only your
own line.)

### 7.2 Where the logs are

| Content | Location |
|---|---|
| Unified log root | `H:\brwork\logs\` (online tasks `BackupRestore-backup-*` / `BackupRestore-restore-*`) |
| Evidence per task | `H:\brwork\tasks\<task-id>\` (task.json / status.json / logs — **all kept**) |
| Recovery environment main log | `Recovery.log` next to the image (e.g. `H:\BRImages\Recovery.log`) |
| GUI action audit | `H:\brwork\logs\gui.log` (every click is recorded; look here first when troubleshooting) |

Prefer not to memorise paths: the top-bar `View logs` lists the **20 most recently modified** log
files under the log root (the current run's log comes first); select one and click "Open in Notepad",
or click "Open folder" to browse in Explorer.

### 7.3 Structured verdict lines (for automation and bug reports)

The log also carries a few **plain-ASCII, fixed-format, language-independent** verdict lines. Scripts
read them; humans can grep them to locate a problem fast:

| Prefix | Where | What it tells you |
|---|---|---|
| `RESTORE_SELFCHECK target= serial_before= serial_after= ntoskrnl=` | Restore finalisation (three places: recovery-environment wrap-up, re-entry into the Windows volume, data volume) | Whether the target serial changed and whether the kernel file exists. **This line is the hard evidence of success**, more trustworthy than "progress 100%" |
| `CAPTURE_HEARTBEAT elapsed_secs= image_bytes= phase=` | Offline capture of a system volume, one line every 30 seconds | The image is still growing, therefore work is happening |
| `CAPTURE_STALL elapsed_secs= image_bytes= silent_secs=` | The image has not grown for 10 minutes during capture | **Not a failure**: DISM pre-scans the whole volume before writing, so zero bytes at the start is normal (measured: a 8.7 GB subtree pre-scanned for 100 s; a whole volume can take minutes) |
| `CAPTURE_SUMMARY` | Written once when capture ends | The totals for that capture |

Do not kill the process just because you see `CAPTURE_STALL`: it only records, it never intervenes.
Real timeouts are handled by `dism_timeout_secs` in Settings.

---

## 8. Known boundaries (repeated on purpose)

- UEFI + GPT + NTFS system partition only; a second system is not possible on an MBR disk.
- No incremental/differential backup, no network backup, no automatic BitLocker handling.
- An overlay restore (formatting unchecked) leaves old files behind; check formatting for a clean
  result.
- Preserving ACLs is the default (owner/ACL restored byte for byte); the overlay channel preserves
  them too.
