import logging
import time
import tomllib
from datetime import datetime
from threading import Thread

from ..clients.client import Client
from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..utils.script import setup_logging
from ..utils.sys_info import create_sys_minicallhome, eth_mac_addr

mac_addr = eth_mac_addr()
assert mac_addr is not None

with open("mqtt-bench.toml", "rb") as f:
    config = tomllib.load(f)

setup_logging(config)

sim7020_conf = config["client"]["sim7020"]
mqtt_conf = config["client"]["mqtt"]

clients: list[Client] = []
if sim7020_conf.get("enable", True):
    logging.info(f"Enabled SIM7020 MQTT client at {sim7020_conf['device']}")
    clients.append(SIM7020MqttClient(mac_addr, sim7020_conf["device"], "SendPunch"))
if mqtt_conf.get("enable", True):
    logging.info("Enabled MQTT client")
    clients.append(MqttClient(mac_addr))


def mini_call_home():
    while True:
        mini_call_home = create_sys_minicallhome()
        for client in clients:
            client.send_mini_call_home(mini_call_home)
        time.sleep(20)


thread = Thread(target=mini_call_home, daemon=True)
thread.start()

for i in range(1000):
    for client in clients:
        client.send_punch(46283, datetime.now(), (i + 1) % 1000, 18)
    time.sleep(12)
