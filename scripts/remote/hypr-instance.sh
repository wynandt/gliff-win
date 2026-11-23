# Newest .socket.sock under the runtime dir, when the signature is unset.
# A previous instance can leave an older socket in a newer directory.
if [ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ]; then
  d="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr"
  s=$(ls -td "$d"/*/.socket.sock 2>/dev/null | head -n 1)
  if [ -n "$s" ]; then
    HYPRLAND_INSTANCE_SIGNATURE=$(basename "$(dirname "$s")")
    export HYPRLAND_INSTANCE_SIGNATURE
    echo "gwin: HYPRLAND_INSTANCE_SIGNATURE=$HYPRLAND_INSTANCE_SIGNATURE (newest socket under $d)" >&2
  else
    echo "gwin: no Hyprland instance socket under $d" >&2
  fi
  unset d s
fi
