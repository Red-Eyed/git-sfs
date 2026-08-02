set -eu

cargo="${CARGO:-cargo}"

"$cargo" install cargo-zigbuild --locked
python3 -m pip install --user ziglang

if [ -n "${GITHUB_PATH:-}" ]; then
  printf '%s\n' "$HOME/.cargo/bin" >> "$GITHUB_PATH"
  printf '%s\n' "$HOME/.local/bin" >> "$GITHUB_PATH"
fi
