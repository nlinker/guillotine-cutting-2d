# pi_solver — Monte Carlo estimation of PI via named pipe

## Project structure

```
pi_solver/          ← Rust project (EXE)
  src/main.rs
  Cargo.toml

vba/
  pi_solver.bas     ← VBA module for Excel
```

## Building the Rust EXE

```powershell
# Requires Rust (rustup.rs) and MSVC toolchain
cd pi_solver
cargo build --release

# EXE will be at:
# pi_solver\target\release\pi_solver.exe
```

## Installing in Excel

1. Create a new workbook and save as `.xlsm` (macro-enabled)
2. `Alt+F11` → `Insert` → `Module`
3. Paste the contents of `vba/pi_solver.bas`
4. Copy `pi_solver.exe` to the same folder as the `.xlsm` file
5. Run macro `RunSolver` (`Alt+F8` → `RunSolver` → `Run`)

## Named pipe protocol

Pipe: `\\.\pipe\pi_solver_progress`

All messages are UTF-8 strings terminated with `\n`:

| Message  | Format |
|----------|--------|
| Progress | `PROGRESS\|<sec>\|<iterations>\|<pi_estimate>\|<error>` |
| Done     | `DONE\|<iterations>\|<final_estimate>` |
| Error    | `ERROR\|<text>` |

## EXE parameters

```
pi_solver.exe [seconds]
```

- no arguments — runs for 60 seconds
- `pi_solver.exe 30` — runs for 30 seconds

## How it works

```
Excel (VBA)                    pi_solver.exe
    │                               │
    │── Shell "pi_solver.exe" ─────►│ CreateNamedPipe
    │                               │ ConnectNamedPipe (waits)
    │── CreateFile (open pipe) ────►│ (client connected)
    │                               │
    │◄── PROGRESS|1|500000|3.14...  │ every second
    │◄── PROGRESS|2|1000000|3.141.. │
    │    ...                        │
    │◄── DONE|30000000|3.1415926..  │
    │                               │
    │ CloseHandle                   │ CloseHandle
```

## Sheet1 cells

| Cell  | Content |
|-------|---------|
| A1/B1 | Status |
| A2/B2 | Time (seconds) |
| A3/B3 | Iteration count |
| A4/B4 | Current PI estimate (10 decimal places) |
| A5/B5 | Error |
| A6/B6 | Path to pi_solver.exe |
