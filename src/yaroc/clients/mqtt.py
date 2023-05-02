import logging
import time
from datetime import datetime
from threading import Lock
from typing import Any, Dict, List, Optional, Tuple

import paho.mqtt.client as mqtt
from attila.atcommand import ATCommand
from attila.atre import ATRuntimeEnvironment
from attila.exceptions import ATRuntimeError, ATScriptSyntaxError, ATSerialPortError

from ..pb.punches_pb2 import Punches
from ..pb.status_pb2 import Disconnected, MiniCallHome, Status
from ..pb.utils import create_coords_proto, create_punch_proto
from .client import Client

BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


def topics_from_mac(mac_address: str) -> Tuple[str, str, str]:
    return (
        f"yaroc/{mac_address}/p",
        f"yaroc/{mac_address}/coords",
        f"yaroc/{mac_address}/status",
    )


class MqttClient(Client):
    """Class for a simple MQTT reporting"""

    def __init__(self, mac_address: str, name: Optional[str] = None):
        def on_connect(client: mqtt.Client, userdata: Any, flags, rc: int):
            del client, userdata, flags
            logging.info(f"Connected with result code {str(rc)}")

        def on_disconnect(client: mqtt.Client, userdata: Any, rc):
            del client, userdata
            logging.error(f"Disconnected with result code {str(rc)}")

        self._message_infos: Dict[int, mqtt.MQTTMessageInfo] = {}

        def on_publish(client: mqtt.Client, userdata: Any, mid: int):
            del client, userdata
            del self._message_infos[mid]
            logging.info(f"Published id={mid}")

        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)

        disconnected = Disconnected()
        if name is None:
            disconnected.client_name = ""
            self.client = mqtt.Client()
        else:
            disconnected.client_name = str(name)
            self.client = mqtt.Client(client_id=name, clean_session=False)
        status = Status()
        status.disconnected.CopyFrom(disconnected)
        self.client.will_set(self.topic_status, status.SerializeToString(), qos=1)

        # NB-IoT is slow to connect
        self.client._connect_timeout = 35
        self.client.message_retry_set(26)
        self.client.max_inflight_messages_set(100)  # bump from 20
        self.client.enable_logger()

        self.client.on_connect = on_connect
        self.client.on_disconnect = on_disconnect
        self.client.on_publish = on_publish
        self.client.connect(BROKER_URL, BROKER_PORT, 35)
        self.client.loop_start()

    def __del__(self):
        self.client.loop_stop()

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> mqtt.MQTTMessageInfo:
        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        return self._send(self.topic_punches, punches.SerializeToString())

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString())

    def send_mini_call_home(self, mch: MiniCallHome) -> mqtt.MQTTMessageInfo:
        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), qos=0)

    def wait_for_publish(self, timeout: float | None = None):
        deadline = None if timeout is None else timeout + time.time()
        for message_info in self._message_infos.values():
            while not self.client.is_connected():
                time.sleep(1.0)

            if message_info.rc == mqtt.MQTT_ERR_SUCCESS:
                remaining = None if deadline is None else deadline - time.time()
                message_info.wait_for_publish(remaining)

    def _send(self, topic: str, message: bytes, qos: int = 1) -> mqtt.MQTTMessageInfo:
        message_info = self.client.publish(topic, message, qos=qos)
        self._message_infos[message_info.mid] = message_info
        if message_info.rc == mqtt.MQTT_ERR_NO_CONN:
            logging.error("Message not sent: no connection")
            # TODO: add to unsent messages
        elif message_info.rc == mqtt.MQTT_ERR_QUEUE_SIZE:
            # this should never happen as the queue size is huuuge
            logging.error("Message not sent: queue full")
        else:
            # TODO: store message_info to inquire later
            logging.info(f"Message sent, id = {message_info.mid}")
        return message_info


class SIM7020Interface:
    # TODO: it should be thread-safe
    def __init__(self, port: str, client_name: str = "SIM7020"):
        self.atrunenv = ATRuntimeEnvironment(False)
        self.atrunenv.configure_communicator(port, 115200, None, "\r", rtscts=False)
        try:
            self.atrunenv.open_serial()
            logging.debug("Opened serial port")
        except ATSerialPortError as err:
            logging.error("Failed to open serial port")
            raise err
        self._client_name = client_name
        self._default_delay = 100
        self._default_timeout = 1
        self._atrunenv_lock = Lock()

        self._send_at("AT+CMEE=2")
        self._send_at("ATE0")
        self._send_at("AT+CMQTSYNC=1")

        # self._disconnected = Disconnected()
        # if name is None:
        #     self._disconnected.client_name = ""
        # else:
        #     self._disconnected.client_name = str(name)
        if self._send_at("AT", "OK") is not None:
            logging.info("SIM7020 is ready")
        else:
            raise ATSerialPortError(
                "Modem not responding"
            )  # TODO: there might be a better exception

    def _send_at(
        self,
        command: str,
        expected: str = "OK",
        collectables: List[str] = [],
        queries: List[str] = [],
        timeout: float | None = None,
    ) -> Tuple[str, list[str]] | None:
        timeout = timeout if timeout is not None else self._default_timeout
        at_command = ATCommand(
            command,
            exp_response=expected,
            delay=self._default_delay,
            tout=timeout,
            collectables=collectables,
        )
        try:
            with self._atrunenv_lock:
                self.atrunenv.add_command(at_command)
                response = self.atrunenv.exec_next()
        except ATRuntimeError as err:
            logging.error(f"Runtime error {err}")
            raise err
        except ATSerialPortError as err:
            logging.error("Failed to open serial port")
            raise err
        except Exception as err:
            logging.error(f"Unknown exception {err}")
            raise err

        if response is None:
            return None

        logging.debug(f"{command}: {response.full_response} {response.response}")
        if response.response is None:
            return None
        # if len(response.full_response) == 0:
        #     # TODO: this often happens before the timeout, looks like a bug in ATtila
        #     response = _exec(command[command_end:])
        #     if response is None:
        #         logging.error("No response")
        #         return None

        answers = []
        for query in queries:
            answers.append(response.get_collectable(query))
        return (response.response, answers)

    def __del__(self):
        mqtt_id = self._detect_mqtt_id()
        if mqtt_id is not None:
            self.mqtt_disconnect(mqtt_id)

    def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self._send_at(f"AT+CMQDISCON={mqtt_id}", "OK")

    def _detect_mqtt_id(self) -> int | None:
        try:
            (_, answers) = self._send_at(
                "AT+CMQCON?",
                f'CMQCON: [0-9],1,"{BROKER_URL}"',
                ["CMQCON: ?{id::[0-9]},1"],
                ["id"],
            )
            if len(answers) == 1:
                return int(answers[0])
            return None
        except Exception:
            return None

    def mqtt_connect(self) -> int | None:
        mqtt_id = self._detect_mqtt_id()
        if mqtt_id is not None:
            return mqtt_id

        if self._send_at("AT+CGREG?", "CGREG: 0,1") is None:
            logging.warning("Not registered yet")
            return None

        # Can APN be set automatically?
        response = self._send_at('AT*MCGDEFCONT="IP","trial-nbiot.corp"', "OK", timeout=35)
        if response is None:
            logging.warning("Can not set APN")
            return None

        res = self._send_at(
            "AT+CMQNEW?",
            f"\\+CMQNEW: [0-9],1,{BROKER_URL}",
            ["CMQNEW: ?{mqtt_id::[0-9]},1"],
            ["mqtt_id"],
        )
        if res is not None:
            self.mqtt_disconnect(int(res[1][0]))

        res = self._send_at(
            f'AT+CMQNEW="{BROKER_URL}","{BROKER_PORT}",60000,100',
            "CMQNEW:",
            ["CMQNEW: ?{mqtt_id::[0-9]}"],
            ["mqtt_id"],
            timeout=35,
        )
        if res is None:
            return None
        (_, answers) = res
        try:
            mqtt_id = int(answers[0])
            # TODO: add will flag and will from disconnected
            # status = Status()
            # status.disconnected.CopyFrom(disconnected)
            opt_reponse = self._send_at(
                f'AT+CMQCON={mqtt_id},3,"{self._client_name}",120,0,0', "OK", timeout=35
            )
            if opt_reponse is not None:
                logging.info(f"Connected to mqtt_id={mqtt_id}")
                return mqtt_id
            else:
                logging.error("MQTT connection unsuccessful")
                return None
        except Exception as err:
            logging.error(f"MQTT connection unsuccessful: {err}")
            return None

    def mqtt_send(self, topic: str, message: bytes, qos: int = 0) -> bool:
        if qos == 1:
            if self._detect_mqtt_id() is None:
                self.mqtt_connect()

        message_hex = message.hex()
        opt_response = self._send_at(
            f'AT+CMQPUB=0,"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"', "OK", timeout=60
        )
        return opt_response is not None

    def get_signal_dbm(self) -> int | None:
        res = self._send_at("AT+CENG?", "CENG", ["CENG: ?{ceng::.*}"], ["ceng"])
        try:
            if res is not None and len(res[1]) == 1:
                ceng_split = res[1][0].split(",")
                return int(ceng_split[6])
            return None
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")
            return None


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    def __init__(self, mac_address: str, port: str, name: Optional[str] = None):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)
        self._at_iface = SIM7020Interface(port, name if name is not None else "SIM7020")
        self._at_iface.mqtt_connect()

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> mqtt.MQTTMessageInfo:
        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        return self._send(self.topic_punches, punches.SerializeToString(), "Punch", qos=1)

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString(), "GPS coordinates")

    def send_mini_call_home(self, mch: MiniCallHome):
        dbm = self._at_iface.get_signal_dbm()
        if dbm is not None:
            mch.signal_dbm = dbm

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), "MiniCallHome", qos=0)

    def _send(self, topic: str, message: bytes, message_type: str, qos: int = 0):
        if self._at_iface.mqtt_send(topic, message, qos):
            logging.info(f"{message_type} sent")
        else:
            # TODO: add to unsent messages if response is ERROR
            logging.error("Message not sent")
