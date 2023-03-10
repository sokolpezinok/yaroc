#!/usr/bin/env python3
from sportident import SIReaderSRR
from time import sleep
from datetime import datetime
import sys
import logging
from connectors.mqtt import SimpleMqttConnector

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

START = 3
FINISH = 4
BEACON_CONTROL = 18
TOPIC = "spe/47"

try:
    if len(sys.argv) > 1:
        # Use command line argument as serial port name
        si = SIReaderSRR(port=sys.argv[1])
    else:
        # Find serial port automatically
        si = SIReaderSRR()
    logging.info(f"Connected to station on port {si.port}")
except:
    logging.error(
        "Failed to connect to an SI station on any of the available serial ports."
    )
    exit()

mqtt_connector = SimpleMqttConnector(TOPIC)
print("Insert SI-card to be read")
while True:
    srr_group = si.poll_punch()
    if srr_group is None:
        sleep(1)
        continue

    data = srr_group.get_data()
    now = datetime.now()
    card_number = data["card_number"]
    for punch in data["punches"]:
        (code, time) = punch
        logging.info(
            f"{card_number} punched {code} at {time}, received after {now-time}"
        )
        mqtt_connector.send_punch(card_number, time, now, code, BEACON_CONTROL)
    if isinstance(data["start"], datetime):
        time = data["start"]
        logging.info(
            f"{card_number} punched start (8) at {time}, received after {now-time}"
        )
        mqtt_connector.send_punch(card_number, time, now, 8, START)
    if isinstance(data["finish"], datetime):
        time = data["finish"]
        logging.info(
            f"{card_number} punched start (8) at {time}, received after {now-time}"
        )
        mqtt_connector.send_punch(card_number, time, now, 10, FINISH)
