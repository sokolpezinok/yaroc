import logging
import time
from datetime import datetime
from threading import Lock
from typing import Any, Dict, List, Optional, Tuple

import paho.mqtt.client as mqtt
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


class SIM7020MqttClient(Client):
    """Class for an MQTT client using SIM7020's AT commands"""

    # TODO: move AT commands into a separate class

    def __init__(self, mac_address: str, port: str, name: Optional[str] = None):
        self.topic_punches, self.topic_coords, self.topic_status = topics_from_mac(mac_address)

        self.atrunenv = ATRuntimeEnvironment(False)
        self.atrunenv.configure_communicator(port, 115200, None, "\r", rtscts=False)
        try:
            self.atrunenv.open_serial()
            logging.debug("Opened serial port")
        except ATSerialPortError as err:
            logging.error("Failed to open serial port")
            raise err

        self._lock = Lock()
        self._atrunenv_lock = Lock()
        self._send_at("AT+CMEE=2;;OK;;300;;1")
        self._send_at("ATE0;;OK;;300;;1")
        if self._send_at("AT;;OK;;300;;1") is not None:
            logging.info("SIM7020 is ready")

        self._disconnected = Disconnected()
        if name is None:
            self._disconnected.client_name = ""
        else:
            self._disconnected.client_name = str(name)
        self._connect()

    def _send_at(self, command: str) -> List[str] | None:
        (_, opt_response) = self._send_at_queries(command, [])
        return opt_response

    def _send_at_queries(self, command: str, queries=[str]) -> Tuple[List[str], List[str] | None]:
        def _exec(command: str):
            try:
                with self._atrunenv_lock:
                    return self.atrunenv.exec(command)
            except ATRuntimeError as err:
                logging.error(f"Runtime error {err}")
                raise err
            except ATSerialPortError as err:
                logging.error("Failed to open serial port")
                raise err
            except Exception as err:
                logging.error(f"Unknown exception {err}")
                raise err

        error_res: Tuple[List[str], List[str] | None] = ([], None)
        try:
            response = _exec(command)
        except Exception:
            return error_res

        if response is None:
            logging.error("No response")
            return error_res

        command_end = command.index(";;")
        orig_command = response.command.command
        if len(response.full_response) == 0:
            # TODO: this often happens before the timeout, looks like a bug in ATtila
            response = _exec(command[command_end:])
            if response is None:
                logging.error("No response")
                return error_res
        logging.debug(f"{orig_command}: {response.full_response} {response.response}")
        if response.response is None:
            return ([], None)

        res = []
        for query in queries:
            res.append(response.get_collectable(query))
        return (res, response.full_response)

    def _detect_mqtt_id(self) -> int | None:
        (answers, full_response) = self._send_at_queries(
            f'AT+CMQCON?;;CMQCON: [0-9],1,"{BROKER_URL}";;200;;2;;'
            '["CMQCON: ?{mqtt_id::[0-9]},1"]',
            ["mqtt_id"],
        )
        try:
            if len(answers) == 1:
                return int(answers[0])
            return None
        except Exception:
            return None

    def __del__(self):
        mqtt_id = self._detect_mqtt_id()
        if mqtt_id is not None:
            self._disconnect(mqtt_id)

    def _disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self._send_at(f"AT+CMQDISCON={mqtt_id};;OK;;200;;5")

    def _connect(self) -> int | None:
        # TODO: move elsewhere
        # self._send_at("AT+CMQTSYNC=1;;OK;;100;;1")
        mqtt_id = self._detect_mqtt_id()
        if mqtt_id is not None:
            return mqtt_id

        if self._send_at("AT+CGREG?;;CGREG: 0,1;;100;;2") is None:
            logging.warning("Not registered yet")
            return None

        # Can APN be set automatically?
        response = self._send_at('AT*MCGDEFCONT="IP","trial-nbiot.corp";;OK;;100;;35')
        if response is None:
            logging.warning("Can not set APN")
            return None

        (answers, full_response) = self._send_at_queries(
            f"AT+CMQNEW?;;\\+CMQNEW: [0-9],1,{BROKER_URL};;100;;2;;"
            '["CMQNEW: ?{mqtt_id::[0-9]},1"]',
            ["mqtt_id"],
        )
        if full_response is not None:
            self._disconnect(int(answers[0]))

        cmqnew = f'AT+CMQNEW="{BROKER_URL}","{BROKER_PORT}",60000,100'
        (answers, full_response) = self._send_at_queries(
            cmqnew + ';;CMQNEW: ;;200;;35;;["CMQNEW: ?{mqtt_id::[0-9]}"]', ["mqtt_id"]
        )
        try:
            mqtt_id = int(answers[0])
            # TODO: add will flag and will from disconnected
            # status = Status()
            # status.disconnected.CopyFrom(disconnected)
            opt_reponse = self._send_at(
                f'AT+CMQCON={mqtt_id},3,"{self._disconnected.client_name}",120,0,0;;OK;;1000;;35'
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

    def send_punch(
        self,
        card_number: int,
        si_time: datetime,
        code: int,
        mode: int,
        process_time: datetime | None = None,
    ) -> mqtt.MQTTMessageInfo:
        with self._lock:
            if self._detect_mqtt_id() is None:
                self._connect()

        punches = Punches()
        punches.punches.append(create_punch_proto(card_number, si_time, code, mode, process_time))
        return self._send(
            self.topic_punches,
            punches.SerializeToString(),
            "Punch",
        )

    def send_coords(
        self, lat: float, lon: float, alt: float, timestamp: datetime
    ) -> mqtt.MQTTMessageInfo:
        coords = create_coords_proto(lat, lon, alt, timestamp)
        return self._send(self.topic_coords, coords.SerializeToString(), "GPS coordinates")

    def send_mini_call_home(self, mch: MiniCallHome):
        (answers, opt_response) = self._send_at_queries(
            'AT+CENG?;;CENG:;;100;;2;;["CENG: ?{ceng::.*}"]', ["ceng"]
        )
        try:
            if opt_response is not None and len(answers) == 1:
                ceng_split = answers[0].split(",")
                mch.signal_dbm = int(ceng_split[6])
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")

        status = Status()
        status.mini_call_home.CopyFrom(mch)
        return self._send(self.topic_status, status.SerializeToString(), "MiniCallHome", qos=0)

    def _send(self, topic: str, message: bytes, message_type: str, qos: int = 1):
        message_hex = message.hex()
        opt_response = self._send_at(
            f'AT+CMQPUB=0,"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}";;OK;;100;;45'
        )
        if opt_response is None:
            # TODO: add to unsent messages if response is ERROR
            logging.error("Message not sent: no connection")
        else:
            logging.info(f"{message_type} sent")
