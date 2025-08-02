# YAROC

Yet Another [ROC](https://roc.olresultat.se). Radio Online Control for orienteering and other sports that use SportIdent timing (trail running, MTB enduro).

[![Python (Linux)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml)
[![Python (Windows)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml)
[![Rust](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml)

It's as if [ROC](https://roc.olresultat.se) and [jSh.radio](http://radio.jsh.de) had a baby.

# Features

* **Very low latency, very low bandwith**. Using a fast medium such as Wi-Fi or LTE allows for latencies around 100 to 200 milliseconds. Bandwidth used during one competition is well bellow 1 MB for each YAROC forest unit. This allows you to use cheap IoT SIM cards.
* **Supports multiple physical layers: radio (LoRa), NB-IoT, LTE, Wi-Fi, LAN**. Low power technologies such as NB-IoT and LoRa are the best solution for remote sport events. The other 3 (LTE, Wi-Fi and LAN) offer minimum latency.
* **Simple integration via USB** recognizable by most orienteering softwares. Just plug in a Raspberry Pi in the finish area, connect it to internet and you are done!
* **ROC-compatible mode**. If you own a ROC device, you can use YAROC instead and it will work almost the same (some features missing).
* **Generator of fake SportIdent punches**: very useful for load testing of the system, for example to determine the right LoRa settings respecing duty cycle limits.
* **Radio mesh**. When using LoRa, the LoRa devices create a mesh network and transmit punches using other LoRa nodes to the finish area. The mesh can connect to the internet via LTE, so placing one node on a hill makes the whole mesh online.
* **Run everywhere: Linux, Windows, Raspberry Pi, microcontrollers**. We're searching for [the right hardware](https://github.com/sokolpezinok/yaroc/issues/6) for NB-IoT but in principle this is just a matter of time when it happens.
* **Open-source**


# Etymology

YAROC is pronounced phonetically as "jarok", which is Slovak for a small ditch. Thus the ISOM map symbol of YAROC is [108 Small erosion gully](https://omapwiki.orienteering.sport/symbols/108-small-erosion-gully/). Symbol 108 will be the logo of the project once I have some time to create one.

# Installation

Install from PyPI.

```sh
python -m venv .venv
source .venv/bin/activate
pip install yaroc
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
source .venv/bin/activate
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

### Configure receiving punches over UART

First, enable the right serial mode.

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

First, create a `yarocd.toml` file where you configure the MAC addresses to receive the punches from, as well as all the clients that should receive the punches: ROC, SIRAP, serial, etc.

TODO: full list of clients

```toml
log_level = "info"

[mac-addresses]
spe01 = "b827eb78912f"

[client.sirap]
enable = true
ip = "192.168.1.10"
port = 10000

[client.roc]
enable = true
```

With a config file present, we are able to run the YAROC daemon called `yarocd`:
```sh
source .venv/bin/activate
yarocd
```

# Development

In order to start developing, install also the `dev` dependencies:

```sh
source .venv/bin/activate
pip install ".[dev]"
pip install -e .
```

The last line installs the package in edit mode, so you can test each file modification immediately.

To use LSPs, also run the following:

```sh
pip install ".[lsp]"
```


# Other projects

* [ROC](https://roc.olresultat.se)
* [jSh.radio](http://radio.jsh.de)
* [WiRoc](https://wiroc.se)
