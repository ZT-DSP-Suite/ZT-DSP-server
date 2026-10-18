#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright (c) 2026 ZT-DSP Suite contributors
#
# Repository guard.
#
# This repository now contains an implementation, so the old guard (which rejected
# any .rs/.toml/.py file and demanded placeholder wording) would reject the project
# itself. It now enforces the things that still matter:
#
#   1. every first-party source file carries an SPDX licence header;
#   2. no source file logs identity material;
#   3. the bilingual mirrors exist and are not empty;
#   4. no placeholder wording survives in a README.

set -uo pipefail

fail=0
note() { printf '  %s\n' "$*"; }
bad()  { printf 'x %s\n' "$*"; fail=1; }

# Tracked + untracked-but-not-ignored, so the guard works before and after `git add`.
files() { git ls-files -co --exclude-standard -- "$@"; }

echo '== 1. licence headers =='
while IFS= read -r f; do
  case "$f" in
    *.rs|*.sh)
      if ! head -5 "$f" | grep -q 'SPDX-License-Identifier'; then
        bad "$f: missing SPDX-License-Identifier"
      fi
      ;;
  esac
done < <(files '*.rs' '*.sh')
if [[ $fail -eq 0 ]]; then note 'all sources carry SPDX headers'; fi

echo '== 2. no identity material in logs =='
if files 'src/*.rs' | xargs -r grep -nE '(info|warn|error|debug|trace)!\([^)]*(token_hash|session_id|\.identity)' 2>/dev/null; then
  bad 'a log macro appears to format identity material'
else
  note 'no log macro formats token hashes or session ids'
fi

echo '== 3. bilingual mirrors =='
for f in README.md cn/README.md en/README.md; do
  if [[ ! -s "$f" ]]; then bad "$f missing or empty"; fi
done
if [[ $fail -eq 0 ]]; then note 'mirrors present and non-empty'; fi

echo '== 4. no placeholder wording left =='
if files 'README.md' 'cn/**' 'en/**' | xargs -r grep -niE 'placeholder repository|尚未开始实现|no implementation yet' 2>/dev/null; then
  bad 'placeholder wording still present in a README'
else
  note 'no placeholder wording'
fi

echo
if [[ $fail -ne 0 ]]; then
  echo 'check.sh: FAILED'
  exit 1
fi
echo 'check.sh: ok'
