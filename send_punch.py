from sportident import SIReaderSRR
from time import sleep
from datetime import datetime
import sys
import logging
from connectors.mqtt import SimpleMqttConnector


BEACON_CONTROL = 18
TOPIC = "spe/47"

try:
    if len(sys.argv) > 1:
        # Use command line argument as serial port name
        si = SIReaderSRR(port=sys.argv[1])
    else:
        # Find serial port automatically
        si = SIReaderSRR()
    print("Connected to station on port " + si.port)
except:
    print("Failed to connect to an SI station on any of the available serial ports.")
    exit()

logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

mqtt_connector = SimpleMqttConnector(TOPIC)
print("Insert SI-card to be read")
while True:
    srr_group = si.poll_punch()
    if srr_group is None:
        sleep(1)
        continue

    data = srr_group.get_data()
    for punch in data["punches"]:
        now = datetime.now()
        (code, time) = punch
        card_number = data["card_number"]
        logging.info(
            f"{card_number} punched {code} at {time}, received after {now-time}"
        )
        mqtt_connector.send(card_number, time, now, code, BEACON_CONTROL)
