#!/usr/bin/env bash
# Installs Ubuntu packages on a GitHub Linux runner: `apt-install.sh <apt-get install args>`.
#
# The runners point apt at Azure's copy of the archive (`azure.archive.ubuntu.com`, through
# `/etc/apt/apt-mirrors.txt`). On 2026-11-11 that name stopped resolving for hours, and Ubuntu's
# own `archive.ubuntu.com` and `security.ubuntu.com` timed out from the runners too, so every job
# that installs a package failed before running anything. apt checks every package against
# Ubuntu's signing keys whatever server it comes from, so the mirror list starts with kernel.org's
# copy of the archive, which Canonical does not run, keeps Ubuntu's own servers after it, and apt
# retries a download that fails.
set -euo pipefail

mirrors=/etc/apt/apt-mirrors.txt
if [ -f "$mirrors" ]; then
  # One mirror per line, its address and its priority separated by a tab.
  printf '%s\t%s\n' \
    'http://mirrors.edge.kernel.org/ubuntu/' 'priority:1' \
    'http://archive.ubuntu.com/ubuntu/' 'priority:2' \
    'http://azure.archive.ubuntu.com/ubuntu/' 'priority:3' |
    sudo tee "$mirrors" >/dev/null
fi

options=(-o Acquire::Retries=5 -o Acquire::http::Timeout=20)
sudo apt-get "${options[@]}" update
sudo apt-get "${options[@]}" install --yes "$@"
