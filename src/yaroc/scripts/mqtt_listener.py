import logging
from datetime import datetime, timezone

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.roc import RocClient
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punch

roc_client = RocClient("b827eb1d3c4f")


def on_connect(client, userdata, flags, rc):
    del userdata, flags
    logging.info(f"Connected with result code {rc}")

    # Subscribing in on_connect() means that if we lose the connection and
    # reconnect then subscriptions will be renewed.
    client.subscribe("yaroc/47/#", qos=1)


def _prototime_to_datetime(prototime: Timestamp) -> datetime:
    return prototime.ToDatetime().replace(tzinfo=timezone.utc).astimezone()


def on_message(client, userdata, msg):
    del client, userdata
    if msg.topic == "yaroc/47/punches":
        punch = Punch.FromString(msg.payload)
        si_time = _prototime_to_datetime(punch.si_time)
        process_time = _prototime_to_datetime(punch.si_time)
        now = datetime.now().astimezone()
        total_latency = now - si_time

        log_message = (
            f"{punch.code:03} {now}, dated {si_time}, processed {process_time}"
            f" latency {total_latency}\n"
        )
        with open("/home/lukas/mqtt.log", "a") as f:
            f.write(log_message)
        logging.info(f"{msg.topic} {log_message}")

        # TODO: make this configurable
        # roc_client.send_punch(punch.card, si_time, now, punch.code, punch.mode)
        return

    if msg.topic == "yaroc/47/coords":
        coords = Coordinates.FromString(msg.payload)
        orig_time = _prototime_to_datetime(coords.time)
        total_latency = datetime.now().astimezone() - orig_time
        log_message = (
            f"{orig_time}: {coords.latitude},{coords.longitude}, altitude "
            f"{coords.altitude}. Latency {total_latency}s.\n"
        )
        with open("/home/lukas/events.log", "a") as f:
            f.write(log_message)
        logging.info(f"{msg.topic} {log_message}")
        return

    message = msg.payload.decode("utf-8")
    split_message = message.split(";")
    if len(split_message) == 2:
        orig_time = datetime.fromisoformat(split_message[1])
        total_latency = datetime.now() - orig_time
        csq = int(split_message[0])
        with open("/home/lukas/events.log", "a") as f:
            f.write(
                f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm, at {orig_time}, "
                f"latency {total_latency}\n"
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

client.loop_forever()
