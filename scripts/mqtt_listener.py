#!/usr/bin/env python3
import requests
import paho.mqtt.client as mqtt
from datetime import datetime
import logging


def on_connect(client, userdata, flags, rc):
    del userdata, flags
    logging.info(f"Connected with result code {rc}")

    # Subscribing in on_connect() means that if we lose the connection and
    # reconnect then subscriptions will be renewed.
    client.subscribe("spe/47", qos=1)


def on_message(client, userdata, msg):
    del client, userdata
    message = msg.payload.decode("utf-8")
    split_message = message.split(";")
    if len(split_message) == 5:
        sitime = datetime.fromisoformat(split_message[3])
        total_latency = datetime.now() - sitime
        now = datetime.now()
        code = int(split_message[0])

        with open("/home/lukas/mqtt.log", "a") as f:
            f.write(
                f"{code:03} {now}, dated {split_message[3]}, "
                f"latency {total_latency}\n"
            )

        data = {
            "control1": split_message[0],
            "sinumber1": split_message[1],
            "stationmode1": split_message[2],
            "date1": sitime.strftime("%Y-%m-%d"),
            "sitime1": sitime.strftime("%H:%M:%S"),
            "ms1": sitime.strftime("%f")[:3],
            "roctime1": split_message[4][:19],
            "macaddr": "b827eb1d3c4f",
            "1": "f",
            "length": str(118 + sum(map(len, split_message[:3]))),
        }

        try:
            response = requests.post(
                "https://roc.olresultat.se/ver7.1/sendpunches_v2.php", data=data
            )
            logging.debug(f"Got response {response.status_code}: {response.text}")
        except requests.exceptions.RequestException as e:
            logging.error(e)

        message = (
            f"{code:03};{split_message[1]};{split_message[2]};"
            f"{sitime};{total_latency};{split_message[4]}"
        )

    if len(split_message) == 4:
        try:
            orig_time = datetime.fromisoformat(split_message[3])
            total_latency = datetime.now() - orig_time
            with open("/home/lukas/events.log", "a") as f:
                f.write(
                    f"{split_message[3]}: {split_message[0]},{split_message[1]}, altitude "
                    f"{split_message[2]}. Latency {total_latency}s.\n"
                )
        except:
            logging.error("Failed to parse time")

    if len(split_message) == 2:
        orig_time = datetime.fromisoformat(split_message[1])
        total_latency = datetime.now() - orig_time
        csq = int(split_message[0])
        with open("/home/lukas/events.log", "a") as f:
            f.write(
                f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm, at {orig_time}, latency {total_latency}\n"
            )
        message = f"{split_message[0]};{split_message[1]};{total_latency}"

    logging.info(f"{msg.topic} {message}")


logging.basicConfig(
    encoding="utf-8",
    level=logging.INFO,
    format="%(asctime)s - %(levelname)s - %(message)s",
)

client = mqtt.Client()
client.on_connect = on_connect
client.on_message = on_message

client.connect("broker.hivemq.com", 1883, 60)

# Blocking call that processes network traffic, dispatches callbacks and
# handles reconnecting.
# Other loop*() functions are available that give a threaded interface and a
# manual interface.
client.loop_forever()
