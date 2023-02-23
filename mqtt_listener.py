import paho.mqtt.client as mqtt
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
