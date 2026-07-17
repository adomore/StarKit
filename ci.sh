#!/usr/bin/env bash
#
# StarKit local CI (task T0-4).
#
#   ./ci.sh           fmt, clippy, tests, oracle, fixture determinism smoke
#   ./ci.sh --full    the above plus the ~6 min fixture AC tests + 61 MP bench
#   ./ci.sh --quick   skip the fixture smoke (no cargo run --release)
#
# Exits non-zero on the first failure. Every step prints what it is checking, so
# a red run says which invariant broke rather than just "failed".
#
# Portability: bash, and works from Git Bash on Windows. The one place the host
# genuinely matters is the manifest check — see PLATFORM NOTE below.

set -uo pipefail

cd "$(dirname "$0")"

RED=$'\033[31m'; GREEN=$'\033[32m'; YELLOW=$'\033[33m'; DIM=$'\033[2m'; OFF=$'\033[0m'
if [ ! -t 1 ]; then RED=""; GREEN=""; YELLOW=""; DIM=""; OFF=""; fi

FULL=0
QUICK=0
for arg in "$@"; do
  case "$arg" in
    --full)  FULL=1 ;;
    --quick) QUICK=1 ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \?//'; exit 0 ;;
    *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
  esac
done

FAILED=()
STEP=0

# Run a step; record failures for the summary and keep going, so one red step
# does not hide the state of the rest.
#
# Output is captured and shown only on failure: a wall of green test output
# trains people to stop reading it, and then the one line that mattered scrolls
# past unnoticed.
run_step() {
  local name="$1"; shift
  STEP=$((STEP + 1))
  local log start rc
  log=$(mktemp 2>/dev/null || mktemp -t starkit-ci)
  start=$(date +%s)
  "$@" >"$log" 2>&1
  rc=$?
  if [ $rc -eq 0 ]; then
    printf '  %s✓%s %-42s %s%ss%s\n' "$GREEN" "$OFF" "$name" "$DIM" "$(( $(date +%s) - start ))" "$OFF"
  else
    printf '  %s✗%s %-42s %s(exit %d)%s\n' "$RED" "$OFF" "$name" "$DIM" "$rc" "$OFF"
    sed 's/^/      /' "$log"
    FAILED+=("$name")
  fi
  rm -f "$log"
  return $rc
}

# ---------------------------------------------------------------------------
# Locate the oracle interpreter (D-021: Scripts/ on Windows, bin/ on POSIX).
# ---------------------------------------------------------------------------
find_python() {
  for candidate in oracle/.venv/bin/python oracle/.venv/Scripts/python.exe; do
    [ -x "$candidate" ] && { echo "$candidate"; return 0; }
  done
  return 1
}

oracle_tests() {
  local py
  if ! py=$(find_python); then
    printf '    %sthe oracle venv is missing.%s A green CI cannot skip the oracle:\n' "$RED" "$OFF"
    printf '      python3 -m venv oracle/.venv\n'
    printf '      oracle/.venv/bin/python -m pip install -r oracle/requirements.txt\n'
    return 1
  fi
  "$py" -m pytest oracle -q
}

# ---------------------------------------------------------------------------
# Fixture determinism smoke: regenerate one suite, verify against the committed
# manifest (T0-4 AC).
#
# PLATFORM NOTE (D-012): byte-identity is guaranteed for the same platform and
# toolchain, which is what INV-2 requires. It is NOT guaranteed across platforms
# — rand_distr routes transcendentals through the host libm — and the committed
# manifest pins the output of the machine that generated it. On a different host
# this step can fail for a reason that is not a bug. It still fails loudly rather
# than being skipped: a silent pass would be worse than an explained failure.
# ---------------------------------------------------------------------------
SMOKE_SUITE=basic-5k

fixture_smoke() {
  # No RETURN trap here: bash's RETURN trap is not function-local, so it would
  # fire again on the caller's return with $tmp out of scope and abort the run
  # under `set -u` — a green CI reporting failure.
  local tmp expected actual rc=0
  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t starkit)

  if ! cargo run --release --quiet -p starkit-fixtures -- \
       gen --suite "$SMOKE_SUITE" --out "$tmp"; then
    rm -rf "$tmp"
    return 1
  fi

  expected=$(grep "  ${SMOKE_SUITE}/" fixtures/expected/MANIFEST.sha256 | sort)
  actual=$(sort "$tmp/MANIFEST.sha256")

  if [ -z "$expected" ]; then
    echo "no lines for ${SMOKE_SUITE} in fixtures/expected/MANIFEST.sha256"
    rc=1
  elif [ "$expected" = "$actual" ]; then
    echo "$(echo "$expected" | wc -l | tr -d ' ') artifacts match the committed manifest"
  else
    echo "regenerated ${SMOKE_SUITE} does not match the committed manifest:"
    diff <(echo "$expected") <(echo "$actual") | head -10
    echo "host: $(uname -s 2>/dev/null || echo unknown)"
    echo "If this is not the host that generated the manifest, see D-012:"
    echo "cross-platform byte-identity is not guaranteed and this may not be a bug."
    rc=1
  fi

  rm -rf "$tmp"
  return $rc
}

# ---------------------------------------------------------------------------
# Performance bench (T1-9): time the full pipeline on the 61 MP variant against
# the committed budget. Generates the fixture on demand if absent (~1 min). The
# failing gate is the 20 s hard cap, which is machine-independent enough to be a
# real regression signal; the 10 s target and the reference number are advisory
# (D-040, same cross-machine reasoning as the manifest note above).
# ---------------------------------------------------------------------------
BENCH_FIXTURE=fixtures/generated/basic-61mp/image.tiff

bench_step() {
  if [ ! -f "$BENCH_FIXTURE" ]; then
    echo "generating basic-61mp (once)..."
    if ! cargo run --release --quiet -p starkit-fixtures -- \
         gen --suite basic-61mp --out fixtures/generated; then
      return 1
    fi
  fi
  cargo run --release --quiet -p starkit-cli -- bench --reps 1
}

# ---------------------------------------------------------------------------

printf '%sStarKit CI%s  %s%s%s\n\n' "$GREEN" "$OFF" "$DIM" "$(uname -s 2>/dev/null || echo unknown)" "$OFF"

run_step "rustfmt"                cargo fmt --all --check
run_step "clippy -D warnings"      cargo clippy --workspace --all-targets -- -D warnings
run_step "cargo test --workspace"  cargo test --workspace
run_step "oracle tests (pytest)"   oracle_tests

if [ "$QUICK" -eq 0 ]; then
  run_step "fixture determinism smoke (${SMOKE_SUITE})" fixture_smoke
else
  printf '%s[-] fixture determinism smoke skipped (--quick)%s\n' "$YELLOW" "$OFF"
fi

if [ "$FULL" -eq 1 ]; then
  # D-011: the full-scale fixture AC tests — regenerate all five suites twice,
  # check two-run byte-identity and the committed manifest. ~6 min.
  run_step "full fixture AC (--ignored)" cargo test --release --quiet -- --ignored
  # T1-9: the 61 MP performance bench against the committed budget.
  run_step "performance bench (61 MP)" bench_step
fi

echo
if [ ${#FAILED[@]} -eq 0 ]; then
  printf '%sall checks passed%s\n' "$GREEN" "$OFF"
  exit 0
fi
printf '%s%d check(s) failed:%s\n' "$RED" "${#FAILED[@]}" "$OFF"
printf '  - %s\n' "${FAILED[@]}"
exit 1
