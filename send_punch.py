from sportident import SIReaderSRR
from time import sleep
from datetime import datetime
import sys
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


mqtt_connector = SimpleMqttConnector(TOPIC)
print("Insert SI-card to be read")
while True:
    srr_group = si.poll_punch()
    if srr_group is None:
        sleep(1)
        continue

    data = srr_group.get_data()
    print("Punch!")
    for punch in data["punches"]:
        now = datetime.now()
        (time, code) = punch
        card_number = data["card_number"]
        mqtt_connector.send(card_number, time, now, code, BEACON_CONTROL)
