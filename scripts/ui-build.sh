#!/usr/bin/env bash
# 作者: Corin-Sands
set -euo pipefail
cd "$(dirname "$0")/../ui"
exec npm run build
