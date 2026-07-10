# YAROC

![yaroc-logo](https://github.com/user-attachments/assets/2765a80f-fc0c-4b2d-97a1-495be607f95a)

Yet Another [ROC](https://roc.olresultat.se). Radio Online Control for orienteering and other sports that use SportIdent timing (trail running, MTB enduro).

[![Python (Linux)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml)
[![Python (Windows)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml)
[![Rust](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml)
[![TypeScript](https://github.com/sokolpezinok/yaroc/actions/workflows/typescript.yaml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/typescript.yaml)

It's as if [ROC](https://roc.olresultat.se) and [jSh.radio](http://radio.jsh.de) had a baby.

# Features

* **Very low latency, very low bandwidth**: Wi-Fi or LTE/LTE-M can achieve latencies as low as 100–200ms. Bandwidth usage under 1 MB per day allows the use of cheap IoT SIM cards. Uses Protobuf for data serialization to minimize packet size.
* **Support for multiple physical layers**: NB-IoT, LTE-M, Radio (LoRa), LTE, Wi-Fi, LAN. Also supports BLE and USB for short-range communication.
* **Radio mesh**: Seamless integration with **Meshtastic** allows for LoRa-based mesh networks. Punches can be hopped across multiple nodes to reach a gateway, which can then bridge the data to the internet or directly to orienteering software (MeOs, etc.).
* **Simple integration via USB** recognizable by most orienteering software. Plug an ethernet cable into a Raspberry Pi in the finish area, connect it via USB cable to a computer and you are done!
* **Broad hardware compatibility**: Runs on everything from Linux machines (Raspberry Pi, PC) to specialized microcontrollers like the nRF52840.
* **Reliability**: Features built-in retries, exponential backoff, and buffering to ensure no punch is lost during network outages.
* **Multiple output protocols**: Integration with ROC, SIRAP, MQTT, and MeOS (MOP) protocols.
* **Generator of fake SportIdent punches**: Very useful for load testing of the system, for example to determine the right LoRa settings respecting duty cycle limits.
* **Open-source**


# Etymology

YAROC is pronounced phonetically as **"jarok"** (/'jarɔk/), which is the Slovak word for a small ditch or minor water channel. 

Reflecting this name, the project's logo is based on the orienteering ISOM map symbol **[306 Minor/seasonal water channel](https://omapwiki.orienteering.sport/symbols/306-minor-seasonal-water-channel/)**.

# Hardware Recommendations

There will be a much more detailed and separate "Hardware recommendation" section later, but here is a short list of recommended setups:

* **Finish Area, running `yarocd`**: [Raspberry Pi](https://rpishop.cz/) with a [Waveshare 2.66inch e-Paper Module](https://www.waveshare.com/2.66inch-e-paper-module.htm?srsltid=AfmBOoomFRnIrLDNmAqFSNwTLLluj7piMe67DC6wXiycHHUCPPDH4UsE) and a [Waveshare CP2102 USB UART Board (Type A)](https://www.waveshare.com/cp2102-usb-uart-board-type-a.htm) to display status and receive punches via USB (directly to MeOS, QuickEvent, etc.). Optionally, include a [RAK6421 Raspberry Pi HAT](https://store.rakwireless.com/products/wisblock-adapter-board-raspberry-pi-rak6421) + [RAK13300](https://store.rakwireless.com/products/rak13300-wisblock-lpwan) to listen to Meshtastic punches directly in `yarocd`.
* **Online Controls (NB-IoT/LTE-M variant), running YAROC nRF52840 firmware**: [RAK Link.One](https://store.rakwireless.com/products/link-one-lte-m-nb-iot-lorawan-device-based-on-nrf52840-sx1262-and-bg77-arduino-ide-compatible?variant=42659406446790), EU868 variant with Unify Enclosure, and a SportIdent SRR sensor connected to the RAK19007 base board UART pins. We recommend using a hybrid LTE-M / NB-IoT SIM card if available. Currently you also need the [RAKDAP1 debug probe](https://store.rakwireless.com/products/daplink-tool), flashing over the USB port is not yet possible (but coming by the end of 2026).
* **Online Controls (LTE/USB Modem or NB-IoT HAT), running `send-punch`**: [Raspberry Pi](https://rpishop.cz/raspberry-pi-2b/5584-recyberry-raspberry-pi-2-model-b-1gb-ram-v11.html) with a USB modem (e.g. Huawei E3372) or a [SIM7020 NB-IoT](https://www.waveshare.com/sim7020e-nb-iot-hat.htm) modem. SportIdent USB SRR dongle in the USB port. We recommend using Raspbery Pi Model 2 (doesn't have Wi-Fi) or Model 3 (has Wi-Fi). Higher models 4 and 5 are unnecessarily power-hungry.
* **Radio Controls (LoRa / radio), running Meshtastic**: [RAK4631 + RAK19007](https://store.rakwireless.com/products/wisblock-starter-kit?variant=41786685096134) (EU868 variant) inside a [Unify Enclosure 100x75x38mm with solar panel](https://store.rakwireless.com/products/unify-enclosure-ip65-100x75x38-solar?variant=42533523587270), with a SportIdent SRR sensor connected to the RAK19007 base board UART pins. Optionally, include a [RAK12500 GPS module](https://store.rakwireless.com/products/rak12500-wisblock-gnss-location-module) for LoRa signal testing before the competition.

# Installation on a Raspberry Pi or a PC

Install the `yaroc` package from PyPI, which provides the `send-punch` and `yarocd` commands. We recommend using [uv](https://docs.astral.sh/uv/getting-started/installation/) for easy installation:

```sh
uv tool install yaroc
```

To install a beta version, use the `--pre` flag:

```sh
uv tool install --pre yaroc
```

Alternatively, you can use `pip` within a virtual environment:

```sh
python -m venv .venv
source .venv/bin/activate  # On Windows use `.venv\Scripts\activate`
pip install yaroc
# or for beta versions:
pip install --pre yaroc
```

# Installation on RAK devices

## RAK Link.One NB-IoT/LTE-M
Setting up the device is currently quite complex, requiring a working Rust toolchain and a debug probe. A simpler method to flash the firmware without compiling or using a debug probe will be available by late 2026.

1. Connect the [RAKDAP1 debug probe](https://store.rakwireless.com/products/daplink-tool) to the Link.One (nRF52840) MCU, following [the official docs](https://docs.rakwireless.com/product-categories/accessories/rakdap1/quickstart/). The probe is used to flash the firmware and view logs. Flashing directly over USB is not currently supported, but is in development.

> [!TIP]
> **Using a Raspberry Pi Debug Probe**
> If you do not have a RAKDAP1, you can use the Raspberry Pi Debug Probe, too. More information will be added later.

2. Install Rust, `rustup`, and `cargo` if you haven't already:
   - **Linux**:
     ```sh
     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
     ```
   - **Windows**: Install rustup and Visual Studio build tools using winget
     ```powershell
     winget install rustup
     winget install --id Microsoft.VisualStudio.2022.BuildTools --silent --override "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
     ```
3. Install `probe-rs` to communicate with the debug probe. We recommend installing it using the official script:
   - **Linux**:
     ```sh
     curl --proto '=https' --tlsv1.2 -LsSf https://github.com/probe-rs/probe-rs/releases/latest/download/probe-rs-tools-installer.sh | sh
     ```
     On Linux, you must also configure `udev` rules to access the debug probe without root privileges:
     ```sh
     sudo curl -L https://probe.rs/files/69-probe-rs.rules -o /etc/udev/rules.d/69-probe-rs.rules
     sudo udevadm control --reload-rules && sudo udevadm trigger
     ```
   - **Windows (PowerShell)**:
     ```powershell
     irm https://github.com/probe-rs/probe-rs/releases/latest/download/probe-rs-tools-installer.ps1 | iex
     ```
   Alternatively, you can install it via Cargo:
   ```sh
   cargo install probe-rs-tools --locked
   ```
4. Set up the Rust toolchain target for the nRF52840 (ARM Cortex-M4F):
   ```sh
   rustup target add thumbv7em-none-eabihf
   ```
5. Clone the repository (if you haven't already), and flash the firmware using Cargo from the project root directory:
   ```sh
   DEFMT_LOG=debug cargo run -p yaroc-nrf52840 --release
   ```
6. This will compile, flash, and run the firmware, displaying the output logs in your terminal. Please refer to the [Send punches using RAK Wireless Link.One](#send-punches-using-rak-wireless-linkone) section to configure the device's network and MQTT parameters.

## LoRa / Meshtastic
> [!NOTE]
> Meshtastic runs on many devices other than RAK Wireless; see [the official list](https://meshtastic.org/docs/hardware/devices/) for all supported hardware.

Follow the [official documentation for nRF52](https://meshtastic.org/docs/getting-started/flashing-firmware/nrf52/).


# Usage

## Configuration Files Location

By default, YAROC commands (`send-punch`, `yarocd` and `yaroc-nrf`) search for their respective configuration files (`send-punch.toml`, `yarocd.toml` and `nrf52840.toml`) in the following locations, in order:

1. **Current Working Directory (pwd)**: The local folder where the command is executed.
2. **Platform Configuration Directory**:
   - **Linux**: Checks `$XDG_CONFIG_HOME/yaroc/` if the environment variable is set, falling back to `~/.config/yaroc/`.
   - **Windows**: Checks `%APPDATA%\yaroc\` (Roaming Application Data), falling back to `%USERPROFILE%\.config\yaroc\`.

## Send punches using RAK Wireless Link.One

When running the nRF52840 firmware on a RAK Link.One device, you can configure the device's IoT network (APN, LTE-M vs. NB-IoT) and MQTT server using the `yaroc-nrf` tool.

The `yaroc-nrf` command (installed automatically as part of the `yaroc` package) reads a TOML configuration file (by default `nrf52840.toml`) and transmits the settings to the connected device over USB serial.

### Configuration File (`nrf52840.toml`)

A template configuration is available at [conf/nrf52840.toml](file:///home/lukas/sokol/yaroc/conf/nrf52840.toml). Here is an example:

```toml
minicallhomenterval = 30           # Mini-call-home status interval in seconds

[modem]
apn = "internet.iot"   # The APN of your SIM card
rat = "NB-IoT"         # Radio Access Technology: "NB-IoT", "LTE-M", or "both"
# rat = "LTE-M"
# rat = "nbiot"        # Dashes are ignored and capitalization does not matter

[modem.bands]
ltem = [3, 8, 20]            # LTE-M frequency bands
nbiot = [3, 8, 20]           # NB-IoT frequency bands

# Optional MQTT configuration (if omitted, default broker `broker.emqx.io` is used)
[mqtt]
url = "mqtt.example.com"
port = 1883
# username = "username"              # Optional MQTT username
# password = "password"              # Optional MQTT password
packet_timeout = 35                  # Packet transmission timeout in seconds
```

### Running the configuration tool

Connect the RAK Link.One board to your computer via USB, and run `yaroc-nrf` specifying the serial port:

```sh
yaroc-nrf --port /dev/ttyACM0
```

By default, the tool will search for the configuration file named `nrf52840.toml` in the default locations (see [Configuration Files Location](#configuration-files-location)). To specify a custom path to the configuration file, use the `--config` option:

```sh
yaroc-nrf --port /dev/ttyACM0 --config /path/to/custom-config.toml
```

## Send punches using LoRa radio

Follow the official [Meshtastic documentation](https://meshtastic.org/docs/introduction/):

1. [Flash firmware](https://meshtastic.org/docs/getting-started/flashing-firmware)
2. [Configure the radio](https://meshtastic.org/docs/configuration/radio/). We recommend using a **private encrypted channel** to avoid unnecessary traffic on public meshes.

    1. Use the [client role](https://meshtastic.org/docs/configuration/radio/device/#role-comparison).
    2. Use the [LOCAL_ONLY rebroadcast mode](https://meshtastic.org/docs/configuration/radio/device/#rebroadcast-mode)
    3. Set **Ok to MQTT** to `true` in the LoRa configuration to allow your packets to be bridged by MQTT:

       ```sh
       meshtastic --set lora.ok_to_mqtt true
       ```

    4. Add a channel named `serial`, it'll be used to transport punches through LoRa. Set **Uplink Enabled** to `true` for the `serial` channel (and any other channel you want to bridge). If your `serial` channel is at index 1:

       ```sh
       meshtastic --ch-index 1 --ch-set uplink_enabled true
       ```
    5. Enable device telemetry (every 5 minutes) to monitor mesh health and battery status:
       ```sh
       meshtastic --set telemetry.device_telemetry_enabled true --set telemetry.device_update_interval 300
       ```

> [!WARNING]
> This is not a bug, but a "feature" of some Meshtastic versions: the telemetry interval is scaled down to 60% for small meshes, so an interval of 5 minutes becomes 3 minutes in reality. To achieve a 5-minute update interval, set it to `500` instead of `300` (see [issue #8619](https://github.com/meshtastic/firmware/issues/8619)).

3. Attach SportIdent's SRR module to a UART pin, a photo will be added later. Configure it using instructions below.

4. Gateway/MQTT configuration: At least one node in the mesh needs to be connected to the internet (via Wi-Fi or Ethernet) to bridge the packets to MQTT.
    1. [Enable MQTT](https://meshtastic.org/docs/configuration/module/mqtt/) in the Meshtastic settings, set the broker URL and root topic to `yar`.

       ```sh
       meshtastic --set mqtt.enabled true --set mqtt.root yar
       ```
    2. Set the **MQTT server** to the one you use in `yarocd.toml` (e.g., `broker.emqx.io`) and the same username and password. Or set it to empty, if not used.

       ```sh
       meshtastic --set mqtt.address broker.emqx.io --set mqtt.username "" --set mqtt.password "" 
       ```



### Configure meshtastic UART

To forward SportIdent's SRR punches over LoRa, we need to configure meshtastic to send them over LoRa. First, enable the right serial mode.

```sh
meshtastic --set serial.mode SIMPLE --set serial.enabled true -set serial.baud BAUD_38400 \
           --set serial.timeout 100
```

Next, configure the correct pins based on the device you own.

#### RAK4631
We recommend using UART1: RXD1 (15) and TXD1 (16).

```sh
meshtastic --set serial.rxd 15 --set serial.txd 16
```

You can also use UART0: RXD0 (19) and TXD0 (20).

#### Lilygo T-Beam
We recommend using RXD 13 and TXD 14 for Lilygo T-Beam.

```sh
meshtastic --set serial.rxd 13 --set serial.txd 14
```

## Send punches from Raspberry Pi (or a PC)

First, create a `send-punch.toml` file where you configure punch sources and clients for sending the punches.

```toml
log_level = "info"
# USB sources are enabled by default: SRR dongle, mini-reader or BSM7-USB
# You can disable them using `punch_source.usb.enable = false`

[punch_source.fake] # Source of fake punches, triggered in regular intervals
enable = true
interval = 8

[client.mqtt]
enable = true
broker_url = "broker.emqx.io"
broker_port = 1883

[client.sim7020]
# Use the Waveshare SIM7020 NB-IoT HAT (linked in Hardware Recommendations) connected via serial UART
enable = false
port = "/dev/serial0"

[meshtastic]
# You can connect a Meshtastic device via USB or TCP and use it as a punch source.
# The meshtastic devices acts as an online gateway for its LoRa mesh.
watch_usb = true  # Defaults to false
# Or connect to meshtasticd over TCP:
# tcp = "127.0.0.1:4403"
# [meshtastic.mac-addresses]
# radio01 = "9e12f8a5"
```

With a config file present, we are able to run `send-punch`:
```
send-punch
```



## Receive punches

First, create a `yarocd.toml` file where you configure the MAC addresses to receive the punches from, as well as all the clients that should send the punches: ROC, SIRAP, serial, etc.

```toml
log_level = "info"

[display]
# You can use a Waveshare e-ink display to show a status table of all YAROC units.
model = "epd2in66"

[mqtt]
broker_url = "broker.emqx.io"
# username = joe
# password = mynameisjoe

[mac-addresses]
sim01 = "b827eb78912e"  # YAROC unit with a SIM card
radio1 = "4e18f7a5"     # Meshtastic node (uses a 32-bit ID, which is 8 hex characters)
radio2 = "7bfaf584"

[meshtastic]
main_channel = "spe" # "SPE" is the shortcut for "Sokol Pezinok", our club name. We use it to name things.

# Meshtastic packets are automatically received via MQTT. You can also connect a Meshtastic
# device via USB or TCP. Disable `watch_usb` to turn off USB device monitoring.
# watch_usb = false
# Or connect to meshtasticd over TCP:
# tcp = "127.0.0.1:4403"

[client.roc]
enable = true

[client.roc.override]
# If you don't have a device registered for ROC, you can remap the device MAC address to
# another one. Useful for meshtastic devices, which can't be registered to ROC directly.
radio1 = "b827eba22867"
radio2 = "b827eba22867"

[client.serial]
# Connect a "UART to USB" board to your Raspberry Pi and receive punches directly into
# orienteering software (MeOS, etc.) over USB.
# Each punch is resent after 1 minute and 10 minutes, because the serial interface does not
# acknowledge receiving punches. The times are not yet configurable.
enable = true
port = "/dev/serial0" # Use "/dev/serial0" on Raspberry Pi

[client.sirap]
# Note: SIRAP is not well tested, use with caution
enable = true
ip = "192.168.1.10"
port = 10000
```

With a config file present, we are able to run the YAROC daemon called `yarocd`:
```sh
yarocd
```

### Advanced: Listening to multiple MQTT servers

For more advanced setups or redundancy, the `yarocd` daemon can listen to multiple MQTT brokers simultaneously. Instead of a single `[mqtt]` table in `yarocd.toml`, you can define multiple brokers using the TOML array of tables syntax `[[mqtt]]`:

```toml
[[mqtt]]
broker_url = "broker.emqx.io"

[[mqtt]]
broker_url = "another-broker.com"
broker_port = 1883
username = "my_user"
password = "my_password"
```

When configured this way, `yarocd` will establish concurrent connections to all defined brokers.

# Development

Note: currently the development instructions are Linux-only. If there's interest, we can also add instructions for Windows. You should be able to figure out most things via quick internet search, though.

## Rust Toolchain Setup

Developing any of the Rust crates, Python bindings, or embedded firmware requires a working Rust toolchain. You can install it via [rustup](https://rustup.rs/):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The workspace defines its compiler toolchain settings in `rust-toolchain.toml` at the root level, which will be selected automatically by Cargo.

## Pure Rust Crates (common, receiver)

The workspace contains pure Rust crates like `common` and `receiver` which are used by both Python bindings and the embedded firmware.

You can develop, build, and test them using standard `cargo` commands from the workspace root:

- Check compilation: `cargo check`
- Run unit tests: `cargo test`
- Build specific packages: `cargo build -p yaroc-common` or `cargo build -p yaroc-receiver`

## nRF52840 Embedded Firmware

Development for the `nrf52840` embedded firmware requires specific hardware and flashing tools.

### Hardware Requirements
A [RAKDAP1 debug probe](https://store.rakwireless.com/products/daplink-tool) (or another compatible SWD debug probe) connected to the target board.

### Software Installation
You must have **`probe-rs`** installed on your development machine for flashing and debugging. You can install it using:
```sh
cargo install probe-rs-tools
```

On Linux, you will also need to configure `udev` rules to access the debug probe without root permissions:
```sh
sudo curl -L https://probe.rs/files/69-probe-rs.rules -o /etc/udev/rules.d/69-probe-rs.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

### Flashing and Running
Since the cargo runner is configured with `probe-rs`, you can compile, flash, and run the firmware with logs outputted directly to your terminal by running:
```sh
cd nrf52840
DEFMT_LOG=debug cargo run --release
```

## Python Bindings & Maturin

In order to start developing the Python parts, install the dependencies using `uv`:

```sh
cd python
uv sync --all-extras
```

This will create a `.venv` and install all extras including `dev` and `lsp`.

### Compiling Rust Bindings (Maturin)
The Python package uses **[maturin](https://www.maturin.rs/)** as its build system to compile the Rust parts (under `python/src/`) into Python modules (specifically `yaroc.rs`).

- Install `maturin` using `uv`: `uv tool install maturin`
- maturin requires the **Rust toolchain** (documented above) to compile the Rust binary parts.
- When running Python scripts/tests through `uv` (e.g. `uv run pytest`), `uv` automatically builds the Rust extension with Maturin behind the scenes.
- You can manually build/install the package in editable mode inside the virtualenv with:
  ```sh
  cd python
  uv run maturin develop -r
  ```

# Other projects

* [ROC](https://roc.olresultat.se)
* [jSh.radio](http://radio.jsh.de)
* [WiRoc](https://wiroc.se)
* [OriLive](https://orilive.live/)
