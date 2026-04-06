# YAROC

![yaroc-logo](https://github.com/user-attachments/assets/2765a80f-fc0c-4b2d-97a1-495be607f95a)

Yet Another [ROC](https://roc.olresultat.se). Radio Online Control for orienteering and other sports that use SportIdent timing (trail running, MTB enduro).

[![Python (Linux)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml)
[![Python (Windows)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml)
[![Rust](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml)

It's as if [ROC](https://roc.olresultat.se) and [jSh.radio](http://radio.jsh.de) had a baby.

# Features

* **Very low latency, very low bandwidth**: Wi-Fi or LTE/LTE-M can achieve latencies as low as 100–200ms. Bandwidth usage under 1 MB per day allows the use of cheap IoT SIM cards. Uses Protobuf for data serialization to minimize packet size.
* **Support for multiple physical layers**: NB-IoT, LTE-M, Radio (LoRa), LTE, Wi-Fi, LAN. Also supports BLE and USB for short-range communication.
* **Radio mesh**: Seamless integration with **Meshtastic** allows for LoRa-based mesh networks. Punches can be hopped across multiple nodes to reach a gateway, which can then bridge the data to the internet or directly to an orienteering software (MeOs, etc.).
* **Simple integration via USB** recognizable by most orienteering softwares. Pluge an ethernet cable into a Raspberry Pi in the finish area, connect it via USB cable to a computer and you are done!
* **Broad hardware compatibility**: Runs on everything from Linux machines (Raspberry Pi, PC) to specialized microcontrollers like the nRF52840.
* **Reliability**: Features built-in retries, exponential backoff, and buffering to ensure no punch is lost during network outages.
* **Multiple output protocols**: Integration with ROC, SIRAP, MQTT, and MeOS (MOP) protocols.
* **Generator of fake SportIdent punches**: very useful for load testing of the system, for example to determine the right LoRa settings respecing duty cycle limits.
* **Open-source**


# Etymology

YAROC is pronounced phonetically as **"jarok"** (/'jarɔk/), which is the Slovak word for a small ditch or minor water channel. 

Reflecting this name, the project's logo is based on the orienteering ISOM map symbol **[306 Minor/seasonal water channel](https://omapwiki.orienteering.sport/symbols/306-minor-seasonal-water-channel/)**.

# Installation

Install the `yaroc` package from PyPI, which provides the `send-punch` and `yarocd` commands. We recommend using [uv](https://docs.astral.sh/uv/getting-started/installation/) for easy installation:

```sh
uv tool install yaroc
```

# Usage

## Send punches from an online control

First, create a `send-punch.toml` file where you configure punch sources and clients for sending the punches.

```toml
log_level = "info"

[punch_source.usb]
enable = true

[punch_source.fake]
enable = true
interval = 8

[client.mqtt]
enable = true
broker_url = "broker.emqx.io"
```

With a config file present, we are able to run `send-punch`:
```
send-punch
```

## Send punches using LoRa radio

Follow the official [Meshtastic documentation](https://meshtastic.org/sk-SK/docs/introduction/):

1. [Flash firmware](https://meshtastic.org/docs/getting-started/flashing-firmware)
2. [Configure the radio](https://meshtastic.org/docs/configuration/radio/), we recommend a private encrypted channel.

    1. Use the [client role](https://meshtastic.org/docs/configuration/radio/device/#role-comparison).
    2. Use the [LOCAL_ONLY rebroadcast mode](https://meshtastic.org/docs/configuration/radio/device/#rebroadcast-mode)
    3. Add a channel named `serial`, it'll be used to transport punches through LoRa.

3. Attach SportIdent's SRR module to a UART port, a photo will be added later. Configure it using instructions below.

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


## Receive punches

First, create a `yarocd.toml` file where you configure the MAC addresses to receive the punches from, as well as all the clients that should send the punches: ROC, SIRAP, serial, etc.

```toml
# "SPE" is the shortcut for "Sokol Pezinok", our club name. We use it to name things,
# so our YAROC units are prefixed "SPE-".

log_level = "info"
# You can use a Waveshare e-ink display to show a status table of all YAROC units.
display = "epd2in66"

[mac-addresses]
spe01 = "b827eb78912e" # YAROC unit with a SIM card
spr01 = "4e18f7a5"     # Meshtastic node (uses a 32-bit ID, which is 8 hex characters)
spr02 = "7bfaf584"

[meshtastic]
main_channel = "spe"
# By default, Meshtastic packets are received via MQTT but you can also connect
# a Meshtastic device using a USB cable.
watch_serial = true

[client.roc]
enable = true

[client.roc.override]
# If you don't have a device registered for ROC, you can remap the device MAC address to
# another one. Useful for meshtastic devices, which can't be registered to ROC directly.
spr01 = "b827eba22867"
spr02 = "b827eba22867"

[client.serial]
# Connect a "UART to USB" board to your Raspberry Pi and receive punches directly
# into orienteering software (MeOS, etc.) over USB.
enable = true
port = "/dev/serial0" # Use "/dev/serial0" on Raspberry Pi (maps to the correct UART)

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

# Development

In order to start developing, install the dependencies using `uv`:

```sh
cd python
uv sync --all-extras
```

This will create a `.venv` and install all extras including `dev` and `lsp`. The package is installed in edit mode by default, so you can test each file modification immediately.


# Other projects

* [ROC](https://roc.olresultat.se)
* [jSh.radio](http://radio.jsh.de)
* [WiRoc](https://wiroc.se)
