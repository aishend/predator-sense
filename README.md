# Predator Sense for the PHN16-72

A Linux version of Acer's PredatorSense, made for the **Acer Predator Helios Neo 16 (PHN16-72)** only. It sends the same firmware calls as the Windows app; those calls were worked out by reverse engineering it (see [docs/REVERSE_ENGINEERING.md](docs/REVERSE_ENGINEERING.md)).

- **Modes:** Quiet, Balanced, Performance, Turbo and Eco. You pick the mode and screen refresh rate (165/60 Hz) to switch to when the charger is plugged in and when it is unplugged. That happens only on plug/unplug and at startup; a mode you choose by hand (app, mode key, Noctalia) is never overridden.
- **Fans:** Automatic, Max, Fixed speed or your own temperature curve.
- **Keyboard RGB:** 4 zones, the 8 firmware effects, brightness, and switching the light off after 30 s without typing.
- **Rear lid logo:** colour and brightness (WMI methods 12/13). `predatorctl set rear_off true|false` switches it off and back on, for idle hooks.
- **Per-mode profiles:** each mode keeps its own fans, keyboard lighting and rear light, and switching mode applies them. The Fan Control and Lighting pages have a "Settings for" bar to edit any mode without switching to it. Quiet and Eco keep the fans on Automatic; in Eco the firmware turns the lights off.
- **Battery:** 80% charge limit, calibration, battery temperature, and USB charging while the laptop is off.
- **GPU:** the Turbo overclock from PredatorSense (+100 MHz core, +200 MHz memory).
- **Other:** boot sound and panel overdrive.
- **Monitoring:** CPU, GPU, system, battery, SSD, Wi-Fi and RAM temperatures, fan RPM, CPU package power, GPU load and RAM.
- **Battery saving:** power-profiles-daemon keeps driving the CPU, but only through the profile matching the mode (Quiet/Eco = power-saver, Turbo = performance, else balanced). The package starts it with `--block-driver platform_profile`, so the firmware mode stays with predator-sensed. On battery the sound card sleeps when silent.
- **Alerts:** a desktop notification when the CPU or GPU passes 90 °C (user service `predator-sense-agent`, which also sets the screen Hz).

## How it fits together

| Part | What it does |
|---|---|
| `kernel/predator_wmi.c` | A small DKMS module that only lets root call a fixed list of Acer WMI methods. It runs next to the in-tree `acer_wmi`, so the hotkeys and `platform_profile` keep working. |
| `predator-sensed` | Root service. It owns all the hardware logic, saves your settings, restores them after boot and resume, and follows mode changes made by the mode key or Noctalia. API: `org.predatorsense.Daemon1` on the system bus. |
| `predatorctl` | Command line client, e.g. `predatorctl mode turbo`. Handy for keybinds. |
| `predator-sense` | GTK4/libadwaita window. |

Changes go through polkit. The user at the active local session is allowed without a password; anyone else must authenticate.

## Install (Arch / CachyOS)

```sh
cd packaging && makepkg -si
```

To try the window without the hardware, run the service against fake firmware on your session bus:

```sh
cargo run -p predator-sensed -- --simulate &
PREDATOR_SIM=1 cargo run -p predator-sense
```
