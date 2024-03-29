# YAROC

Yet Another [ROC](https://roc.olresultat.se). Radio Online Control for orienteering and other sports that use SportIdent timing (trail running, MTB enduro).

[![Python (Linux)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/linux-python.yml)
[![Python (Windows)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/windows-python.yml)
[![Rust](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml/badge.svg)](https://github.com/sokolpezinok/yaroc/actions/workflows/rust.yml)

It's as if [ROC](https://roc.olresultat.se) and [jSh.radio](http://radio.jsh.de) had a baby.

# Features

* **Very low latency, very low bandwith**. The software adds minimal latency on top of the latency of transport layers. Using a fast medium such as Wi-Fi or LTE allows for latencies around 100 to 200 milliseconds. Bandwidth used during one competition is well bellow 1 MB for each YAROC unit in the forest (allows for cheap IOT SIM cards).
* **Combines multiple physical layers: radio (LoRa), NB-IoT, LTE, Wi-Fi, LAN**. As 2G and 3G networks are being [slowly shut down](https://onomondo.com/blog/2g-3g-sunset), we need to transition to technologies such as NB-IoT and LoRa in the forest.
* **Simple integration via a USB connection** recognizable by most orienteering softwares. Just plug in a Raspberry Pi in the finish area, connect it to the internet and you are done!
* **ROC-compatible mode**. If you own a ROC device, you can use YAROC instead and it will work almost the same (some features missing).
* **Generator of fake SportIdent punches**: very useful for load testing of the system, for example to determine the right LoRa settings that don't exceed the duty cycle limits.
* **Radio mesh**. When using LoRa, the LoRa devices create a mesh network and transmit punches using other LoRa nodes to the finish area. The mesh can connect to the internet via LTE, so placing one node on a hill makes the whole mesh online.
* **Run everywhere: Linux, Windows, Raspberry Pi, microcontrollers**. We're searching for [the right hardware](https://github.com/sokolpezinok/yaroc/issues/6) for NB-IoT but in principle this is just a matter of time when it happens.
* **Open-source**


# Etymology

YAROC is pronounced phonetically as "jarok", which is Slovak for a small ditch. Thus the ISOM map symbol of YAROC is [108 Small erosion gully](https://omapwiki.orienteering.sport/symbols/108-small-erosion-gully/). Symbol 108 will be the logo of the project once I have some time to create one.

# Installation

Install from TestPyPi. The package will be published to the main PyPi in the spring of 2024.

```sh
python -m venv .venv
source .venv/bin/activate
pip install --index-url https://test.pypi.org/simple/ --extra-index-url https://pypi.org/simple
```

TODO: install from PyPi

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
```

With a config file present, we are able to run `send-punch`:
```
source .venv/bin/activate
send-punch
```

## Send punches using LoRa radio
TODO: add meshtastic info

## Receive punches

First, create a `mqtt-forwarder.toml` file where you configure the MAC addresses to receive the punches from as well as all the clients that should receive the punches: ROC, SIRAP, serial, etc.

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

With a config file present, we are able to run `mqtt-forwarder`:
```sh
source .venv/bin/activate
mqtt-forwarder
```

# Development

In order to start developing, install also the `test` and `dev` dependencies:

```sh
source .venv/bin/activate
pip install ".[dev]"
pip install ".[test]"
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
