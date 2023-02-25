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
    # TODO: calculate latency
    message = msg.payload.decode("utf-8")
    split_message = message.split(";")
    if len(split_message) == 5:
        sitime = datetime.fromisoformat(split_message[3])
        total_latency = datetime.now() - sitime
        message = f"{split_message[0]};{split_message[1]};{split_message[2]};{sitime};{total_latency};{split_message[4]}"

    if len(split_message) == 4:
        with open("/home/lukas/gps.log", "a") as f:
            f.write(
                f"{split_message[0]},{split_message[1]} ({split_message[2]} meter alt.) at {split_message[3]}\n"
            )

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
