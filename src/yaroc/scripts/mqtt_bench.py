import logging
import time
from datetime import datetime
from threading import Thread

from ..clients.mqtt import MqttClient, SIM7020MqttClient
from ..utils.sys_info import create_minicallhome, eth_mac_addr

logging.basicConfig(
    encoding="utf-8",
    level=logging.DEBUG,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

mac_addr = eth_mac_addr()
assert mac_addr is not None
# mqtt_client = MqttClient(mac_addr, name="benchmark")
mqtt_client = SIM7020MqttClient(mac_addr, "/dev/ttyUSB0", "SIM7020")


def mini_call_home():
    for i in range(1000):
        mini_call_home = create_minicallhome()
        mqtt_client.send_mini_call_home(mini_call_home)
        time.sleep(30)


thread = Thread(target=mini_call_home, daemon=True)
thread.start()

for i in range(1000):
    mqtt_client.send_punch(46283, datetime.now(), (i + 1) % 1000, 18)
    time.sleep(20)
