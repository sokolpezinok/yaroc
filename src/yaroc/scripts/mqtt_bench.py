import logging
import time
from datetime import datetime

from ..clients.mqtt import SimpleMqttClient
from ..utils.sys_info import eth_mac_addr

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

mac_addr = eth_mac_addr()
assert mac_addr is not None
mqtt_client = SimpleMqttClient(mac_addr, name="benchmark")

for i in range(10):
    mqtt_client.send_punch(46283, datetime.now(), (i + 1) % 1000, 18)
    time.sleep(5)

mqtt_client.wait_for_publish(60.0)
