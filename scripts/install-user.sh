#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
data_home="${XDG_DATA_HOME:-${HOME}/.local/share}"
config_home="${XDG_CONFIG_HOME:-${HOME}/.config}"
local_bin="${HOME}/.local/bin"
fcitx_lib="${HOME}/.local/lib/fcitx5"

cargo build --manifest-path "${project_root}/Cargo.toml" --release
cmake -S "${project_root}/fcitx5-addon" \
  -B "${project_root}/fcitx5-addon/build" \
  -G Ninja \
  -DCMAKE_BUILD_TYPE=Release
cmake --build "${project_root}/fcitx5-addon/build"

install -Dm755 "${project_root}/target/release/natsu-typelessd" \
  "${local_bin}/natsu-typelessd"
install -Dm755 "${project_root}/target/release/natsu-typelessctl" \
  "${local_bin}/natsu-typelessctl"
install -Dm755 "${project_root}/fcitx5-addon/build/libnatsutypeless.so" \
  "${fcitx_lib}/libnatsutypeless.so"
mkdir -p "${data_home}/fcitx5/addon"
rm -f "${data_home}/fcitx5/addon/natsu-typeless.conf"
sed "s|^Library=libnatsutypeless$|Library=${fcitx_lib}/libnatsutypeless|" \
  "${project_root}/fcitx5-addon/natsutypeless.conf" \
  > "${data_home}/fcitx5/addon/natsutypeless.conf"
install -Dm644 "${project_root}/data/io.github.ddy314.NatsuTypeless.xml" \
  "${data_home}/natsu-typeless/io.github.ddy314.NatsuTypeless.xml"
for vocabulary_file in "${project_root}/data/vocabulary/"*; do
  install -Dm644 "${vocabulary_file}" \
    "${data_home}/natsu-typeless/vocabulary.d/$(basename "${vocabulary_file}")"
done

worker_source="${data_home}/natsu-typeless/worker"
mkdir -p "${worker_source}"
cp -a "${project_root}/worker/pyproject.toml" "${worker_source}/"
cp -a "${project_root}/worker/uv.lock" "${worker_source}/"
rm -rf "${worker_source}/src"
cp -a "${project_root}/worker/src" "${worker_source}/"

unit_dir="${config_home}/systemd/user"
dbus_dir="${data_home}/dbus-1/services"
mkdir -p "${unit_dir}" "${dbus_dir}"
sed \
  -e "s|/usr/lib/natsu-typeless/natsu-typelessd|${local_bin}/natsu-typelessd|g" \
  -e "s|%h/.local/share/natsu-typeless/venv/bin/python|${data_home}/natsu-typeless/venv/bin/python|g" \
  "${project_root}/data/systemd/user/natsu-typeless.service" \
  > "${unit_dir}/natsu-typeless.service"
sed "s|/usr/lib/natsu-typeless/natsu-typelessd|${local_bin}/natsu-typelessd|g" \
  "${project_root}/data/dbus-1/services/io.github.ddy314.NatsuTypeless.service" \
  > "${dbus_dir}/io.github.ddy314.NatsuTypeless.service"

systemctl --user daemon-reload
systemctl --user enable --now natsu-typeless.service

echo "Installed Natsu Typeless for the current user."
echo "Next: natsu-typelessctl setup"
echo "Then: natsu-typelessctl key set"
echo "Finally restart fcitx5 and configure the Natsu Typeless addon."
