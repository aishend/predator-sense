# PredatorSense 5.0.1463 — protocol notes

Sources — only official ones:

1. Acer's Windows package `PredatorSense_5.0.1463` (PHN16-72 build), analysed
   with Ghidra 12.1.4 and capstone: every encoding and rule below.
2. The firmware of a Predator PHN16-72 (BIOS V1.20): the embedded WMI MOF
   (BMOF) for method IDs and parameter layouts, and the Acer OEM SMBIOS tables.

No third-party Linux driver was used as a source.

## 1. Architecture of the Windows software

```
PredatorSense.exe (Electron UI, app.asar)
   │  TCP 127.0.0.1:46933   "ACER" + u32 packet id + JSON   (PSSdkAdapter)
   ▼
AcerAgentService.exe   — feature logic (PSSocket::*), fan grouping, keyboard,
   │                     model INI files, registry state
   │  named pipe \\.\pipe\predatorsense_hardware_service_   (kSvcCmd*)
   ▼
AcerHardwareService.exe — executes WMI calls, NVAPI GPU OC, SMBIOS parsing
AcerLightingService.exe — RGB devices (OpenRGB based), EC keyboard
AcerQAAgent / AcerCCAgent — battery health, USB power-off charging (websocket)
   │  IWbemServices::ExecMethod   ROOT\WMI
   ▼
Firmware WMI classes: AcerGamingFunction, APGeAction, BatteryControl
```

UI packet ids: `0 INITIALIZATION, 10 GET_MONITOR_DATA, 20 GET_UPDATED_DATA,
30 GET_CONNECTED_DEVICE, 40 GET_LIGHTING_EFFECT_CAPABILITY, 100 SET_DEVICE_DATA`.

The Linux port mirrors this split: `predator_wmi` (kernel) = Windows WMI
provider, `predator-sensed` = AgentService + HardwareService + parts of the
lighting/QA agents, `predator-sense` = UI.

## 2. WMI interfaces

| Class | GUID | Linux iface |
|---|---|---|
| AcerGamingFunction | 7A4DDFE7-5B5D-40B4-8595-4408E0CC7F56 | 0 |
| APGeAction | 61EF69EA-865C-4BC3-A502-A0DEBA0CB531 | 1 |
| BatteryControl | 79772EC5-04B1-4BFD-843C-61E7F77B6CC9 | 2 |

Argument widths follow the MOF: uint32 inputs are marshalled as 4 bytes,
uint64 as 8 bytes, `uint8[16]` as 16 bytes.

**Result convention (AcerGamingFunction):** byte 0 of every integer result is a
status (0 = OK) and the payload starts at bit 8. Every caller in
AcerAgentService tests `(char)result == 0` and then shifts right by 8.

### Method IDs (from the firmware BMOF)

Windows resolves methods by name through the MOF that the firmware embeds as a
compressed binary MOF (WMI object `05901221-D566-11D1-B2F0-00A0C9062910`,
`/sys/bus/wmi/devices/05901221-…/bmof`). The blob is `FOMB` + version +
compressed/uncompressed sizes, followed by Microsoft "DS" compression (an
LZ77/MRCI variant: literals `01`+7 bits / `10`+7 bits | 0x80, matches `00`+6-bit,
`110`+8-bit+64, `111`+12-bit+320 offset, gamma-coded length, offset 4415 = block
end). `tools/bmof-verify.py` decompresses it and checks `daemon/MethodIds.h`.

The class ↔ GUID binding comes from the class `guid` qualifiers in the same MOF.
Values read on the PHN16-72 BIOS V1.20 (all match the code):

| Method | ID |
|---|---|
| SetGamingProfile / GetGamingProfile | 1 / 3 |
| SetGamingLED / GetGamingLED | 2 / 4 |
| GetGamingSysInfo | 5 |
| SetGamingRgbKb / GetGamingRgbKb | 6 / 7 |
| SetGamingFanBehavior / GetGamingFanBehavior | 14 / 15 |
| SetGamingFanSpeed / GetGamingFanSpeed | 16 / 17 |
| SetGamingKBBacklight / GetGamingKBBacklight | 20 / 21 |
| SetGamingMiscSetting / GetGamingMiscSetting | 22 / 23 |
| APGeAction SetFunction / GetFunction | 1 / 2 |
| BatteryControl GetBatteryHealthControlStatus / SetBatteryHealthControl | 20 / 21 |

Parameter declarations from the MOF (in order of the `ID` qualifier):

| Method | in | out |
|---|---|---|
| SetGamingKBBacklight | `uint8 gmInput[16]` | `uint32 gmOutput` |
| GetGamingKBBacklight | `uint32 gmInput` | `uint8 gmReturn`, `uint8 gmOutput[15]` |
| SetGamingLED | `uint8 gmInput[16]` | `uint32 gmOutput` |
| Integer methods | `uint32` (Get*) / `uint64` (Set*) `gmInput` | `uint64` (Get*) / `uint32` (Set*) `gmOutput` |
| APGeAction SetFunction / GetFunction | `uint64` / `uint32 uiInput` | `uint32` / `uint64 uiOutput` |
| GetBatteryHealthControlStatus | `uint8 uBatteryNo, uFunctionQuery, uReserved[2]` | `uint8 uFunctionList, uReturn[2], uFunctionStatus[5]` |
| SetBatteryHealthControl | `uint8 uBatteryNo, uFunctionMask, uFunctionStatus, uReservedIn[5]` | `uint16 uReturn, uint16 uReservedOut` |

## 3. Operating mode

* Set: `SetGamingMiscSetting((mode << 8) | 0x0B)` — `TsClientCommandProcessor::set_operation_mode`.
* Get: `GetGamingMiscSetting(0x0B)` → byte 1.
* Capability: `GetGamingMiscSetting(0x0A)` → byte 1 is a bitmap, bit *n* = mode *n*.
  If the query fails the service offers `{0,1,4,5,6}` (table at 0x14008cfd0).
* Values: `0 Quiet, 1 Balanced(DEFAULT), 4 Performance(EXTREME), 5 Turbo, 6 Eco`.
* After a mode change the service calls `gpu_set_oc_level(level)`
  (disassembly 0x14000d34b–0x14000d37b): Balanced→1, Performance→2, Turbo→3,
  Eco→0, Quiet→0 then 1 (falls through; net level 1).
* OC level indexes the model INI `[OC_GPU]` (Quiet/Balanced/Performance/Turbo,
  `RAM_*`). Values are MHz; the service multiplies by 1000.0 (constant at
  0x14002f410) for NVAPI kHz. PHN16-72 + RTX 4060 (`DEV_28E0&SUBSYS_17311025`):
  Turbo +100 MHz core, +200 MHz memory, other modes 0.

UI rules (Angular widget bundle):
* On battery only Balanced or Eco are offered (`onBatteryPowerMode` filter);
  the profile keeps `opMode` (AC) and `dcMode` (DC, default Balanced). Adaptor
  unplug locks to `dcMode`; plug-in restores `opMode`.
* Selecting Quiet, or leaving Quiet, forces fan mode Auto.
* Turbo remembers `LastOpMode` (last non-turbo mode).

## 4. Sensors

`GetGamingSysInfo(u32)`:
* `0` → supported-sensor bitmap in bits 24..39; bit *k* = sensor id *k+1*.
* `(id << 8) | 1` → reading in bits 8..23 (°C or RPM). From `PSSocket::Get_*`:

| id | sensor |
|---|---|
| 0x01 | CPU temperature |
| 0x02 | CPU fan |
| 0x03 | system temperature |
| 0x04 | system fan 1 (the service actually sends 0x201 here — Acer bug) |
| 0x06 / 0x09 | GPU fan 1 / 2 |
| 0x08 | system fan 2 |
| 0x0A / 0x0B | GPU temperature 1 / 2 |

* `2` → battery boost status in bits 40..47.

Fan amount (`PSSocket::Get_Fan_Amount`): INI `[FanSupport]` if present,
otherwise bitmap bits 0x002 CPU, 0x020/0x100 GPU, 0x008/0x080 system.

## 5. Fans

`SetGamingFanBehavior(u64)`: bits 0..15 fan bitmap (bit 0 CPU, bit 3+n GPU n),
bits 16.. behaviour per fan in 2-bit slots (CPU at 16, GPU n at 22+2n):
`1 auto, 2 max, 3 custom`.

* Auto / Max: one call with every fan set to 1 or 2.
* Custom (`FUN_14003d410`): fans with `fan_custom_auto` go in one call with
  behaviour 1, the rest in a second call with behaviour 3, then
  `SetGamingFanSpeed((percent << 8) | fanId)` for each custom fan,
  `fanId = 1` CPU, `4 + n` GPU n.
* Read back: `GetGamingFanBehavior(bitmap)` (CPU behaviour in bits 8..9) and
  `GetGamingFanSpeed(fanId)` → percent in byte 1.

## 6. EC keyboard lighting (4-zone RGB)

`SetGamingKBBacklight(uint8[16])` (AcerECKeyboardController::SetMode,
ECKeyboard::SetKeyboardBacklightEffect):

| byte | meaning |
|---|---|
| 0 | effect: 0 static, 1 breathing, 2 neon, 3 wave, 4 shifting, 5 zoom, 6 meteor, 7 twinkling |
| 1 | speed 1..9 |
| 2 | brightness 0..100 |
| 3 | 0 |
| 4 | direction: 1 right, 2 left |
| 5..7 | R, G, B |
| 8 | 3 |
| 9 | device: 1 keyboard (3 all/global, 2 light bar) |

Static per-zone colours (ECKeyboard::SetKeyboardLEDBehavior, AcerLightingService
0x14000b3c0), keyboard device:

1. `SetGamingLED(buffer)` with `[0] = 8` (keyboard) and `[5]` = mask of the zones
   whose colour is not black (bit n = zone n+1). The buffer length depends on the
   SMBIOS type 172 header (byte 4 = 2): version ≥ 0x5B → 16 bytes, 0x56..0x5A →
   12 bytes (helper 0x14001e4b0 sends `flag * 4 + 12` bytes). Without the table the
   service sends the integer `0x00000F0000000008` (all four zones).
2. Static effect at the wanted brightness (`SetGamingKBBacklight`, effect 0).
3. Per zone `SetGamingRgbKb((B << 24) | (G << 16) | (R << 8) | (1 << zone))`.

Dynamic effects only use `SetGamingKBBacklight`.

Mode flags from the OpenRGB-style mode table in the DLL: colour on static,
breathing, shifting, zoom, meteor, twinkling; direction on wave and shifting.

Keyboard backlight auto-off after 30 s (EC keyboards, `FUN_14003de10`):
* get: `APGe GetFunction((bk << 8) | 0x80001)`; byte 4 brightness, byte 5 timeout.
* GetGamingKBBacklight returns `gmReturn` followed by `gmOutput[15]` in the
  setter's byte layout.
* set: `SetFunction((bk << 8) | 0x80002 | (((on ? 0x1E : 0) << 8) | brightness) << 32)`
  keeping the current brightness (100 if unreadable).
* `bk` = `BK_Hotkey_Number` from SMBIOS type 170 (see 9).

## 7. Other settings

| Feature | Get | Set |
|---|---|---|
| LCD overdrive | `GetGamingProfile(0)` byte 6 (`0xFF` = unsupported, 1 = on) | `SetGamingProfile(0x10 \| on<<48)` |
| Windows key (EC kbd) | `GetGamingProfile(0)` byte 3 | `SetGamingProfile(0x02 \| on<<24)` |
| Boot animation/sound | `GetGamingMiscSetting(6)`: byte1 1/0 | `SetGamingMiscSetting(6 \| on<<8)` |
| GPU mode (MUX) | `GetGamingMiscSetting(2)`: byte1 − 1 | `SetGamingMiscSetting(((m+1)<<8) \| 2)` |

## 8. Battery and USB charging (AcerQAAgent / AcerCCAgent)

* Power-off USB charging, APGeAction function 4:
  `GetFunction(4)` → byte 1 state (`0x0F` enabled, `0x1F` disabled), byte 2
  threshold (10/20/30 %). `SetFunction((threshold << 16) | (state << 8) | 4)`.
  The UI treats switch and threshold separately; each write keeps the other.
* BatteryControl:
  `GetBatteryHealthControlStatus{uBatteryNo=1, uFunctionQuery=1, uReserved[2]}` →
  `{uFunctionList, uReturn[2], uFunctionStatus[5]}`; status[0] = 80 % limit,
  status[1] = calibration.
  `SetBatteryHealthControl{uBatteryNo=1, uFunctionMask, uFunctionStatus, uReservedIn[5]}`,
  mask 1 = health (80 %), 2 = calibration.

## 9. SMBIOS OEM tables

* Type 172 (0xAC): 6-byte header (byte 4 table type, byte 5 table version), then
  3-byte entries `{fn u8, value u16}`: `0x08` KeyBoardArea (0 per-key USB,
  N zones), `0x0A` KeyBoardColor (1 single, 2 RGB), `0x17` Keyboard_Type
  (1 EC, 2 USB).
* Type 170 (0xAA): 14-byte header, then 4-byte entries `{number u8, ?, type u16}`;
  keyboard-backlight hotkey = entry with `0x80 < number < 0xA0` and type 8.

PHN16-72 BIOS V1.20 (`tests/data/phn16-72`): type 172 type 2 / version 0x5B,
4 zones, RGB, EC keyboard; type 170 backlight hotkey 0x84.

## 10. Things other Linux tools get wrong

Behaviour that exists in Acer's code and is reproduced here:

* the zone-mask `SetGamingLED` step before static per-zone colours;
* the backlight timeout keeps the current brightness;
* power-off USB charging has an independent switch (0x0F/0x1F) and threshold;
* fan bitmaps are built from the detected fan count, auto and custom fans are
  sent as separate groups;
* separate AC/DC modes with the battery lock, Quiet → Auto fans, the per-mode
  GPU clock profile;
* `SetBatteryHealthControl` returns a 16-bit `uReturn`.
