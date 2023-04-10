import logging
from datetime import datetime, timezone

import paho.mqtt.client as mqtt
from google.protobuf.timestamp_pb2 import Timestamp

from ..clients.roc import RocClient
from ..pb.coords_pb2 import Coordinates
from ..pb.punches_pb2 import Punch
from ..pb.status_pb2 import Status

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
    now = datetime.now().astimezone()
    if msg.topic == "yaroc/47/punches":
        punch = Punch.FromString(msg.payload)
        si_time = _prototime_to_datetime(punch.si_time)
        process_time = _prototime_to_datetime(punch.si_time)
        total_latency = now - si_time

        log_message = (
            f"{punch.code:03} dated {si_time}, processed {process_time},"
            f" latency {total_latency}"
        )
        with open("/home/lukas/mqtt.log", "a") as f:
            f.write(f"{log_message}\n")
        logging.info(log_message)

        # TODO: make this configurable
        # roc_client.send_punch(punch.card, si_time, now, punch.code, punch.mode)
        return

    if msg.topic == "yaroc/47/coords":
        coords = Coordinates.FromString(msg.payload)
        orig_time = _prototime_to_datetime(coords.time)
        total_latency = now - orig_time
        log_message = (
            f"{orig_time}: {coords.latitude},{coords.longitude}, altitude "
            f"{coords.altitude}. Latency {total_latency}s."
        )
        with open("/home/lukas/events.log", "a") as f:
            f.write(f"{log_message}\n")
        logging.info(log_message)
        return

    if msg.topic == "yaroc/47/status":
        status = Status.FromString(msg.payload)
        oneof = status.WhichOneof("msg")
        if oneof == "disconnected":
            logging.info(f"Disconnected {status.disconnected.client_name}")
        elif oneof == "signal_strength":
            signal_strength = status.signal_strength
            orig_time = _prototime_to_datetime(signal_strength.time)
            total_latency = now - orig_time
            csq = signal_strength.csq
            log_message = (
                f"{datetime.now()}: CSQ {csq}, {-114 + 2*csq} dBm, at {orig_time}, "
                f"latency {total_latency}"
            )
            with open("/home/lukas/events.log", "a") as f:
                f.write(f"{log_message}\n")
            logging.info(log_message)


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
