import logging
import time
from datetime import datetime, timedelta

from attila.atcommand import ATCommand
from attila.atre import ATRuntimeEnvironment
from attila.exceptions import ATRuntimeError, ATScriptSyntaxError, ATSerialPortError

from ..pb.status_pb2 import Disconnected, Status

# TODO: either share these constants or make them parameters
BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883
CONNECT_TIME = 35


def time_since(t: datetime, delta: timedelta) -> bool:
    return datetime.now() - t > delta


class SIM7020Interface:
    """An AT interface to the SIM7020 NB-IoT chip

    Implements mostly MQTT functionality

    Note: this class is not thread-safe.
    """

    def __init__(self, port: str, will_topic: str, client_name: str = "SIM7020"):
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
        self._mqtt_id: int | None = None
        self._mqtt_id_timestamp = datetime.now()
        self._last_success = datetime.now()

        self._send_at("AT+CMEE=2")
        self._send_at("ATE0")
        self._send_at("AT+CREVHEX=1")
        self._send_at("AT+CMQTSYNC=1")

        status = Status()
        disconnected = Disconnected()
        disconnected.client_name = client_name
        status.disconnected.CopyFrom(disconnected)
        self._will = status.SerializeToString()
        self._will_topic = will_topic
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
        collectables: list[str] = [],
        queries: list[str] = [],
        timeout: float | None = None,
    ) -> list[str] | None:
        timeout = timeout if timeout is not None else self._default_timeout
        at_command = ATCommand(
            command,
            exp_response=expected,
            delay=self._default_delay,
            tout=timeout,
            collectables=collectables,
        )
        try:
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

        if command != response.command.command:
            logging.warning("Response to a different command")
            logging.debug(
                f"{command}/{response.command.command}: {response.full_response} {response.response}"
            )
        else:
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
        return answers

    def __del__(self):
        mqtt_id = self._detect_mqtt_id()
        self.mqtt_disconnect(mqtt_id)

    def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self._send_at(f"AT+CMQDISCON={mqtt_id}", "OK")

    def _detect_mqtt_id(self) -> int | None:
        self._mqtt_id = None
        try:
            answers = self._send_at(
                "AT+CMQCON?",
                f'CMQCON: [0-9],1,"{BROKER_URL}"',
                ["CMQCON: ?{id::[0-9]},1"],
                ["id"],
            )
            if answers is not None and len(answers) == 1:
                self._mqtt_id = int(answers[0])
        finally:
            return self._mqtt_id

    def mqtt_connect(self):
        self._mqtt_id = self._mqtt_connect_internal()
        self._mqtt_id_timestamp = datetime.now()

    def _mqtt_connect_internal(self) -> int | None:
        self._send_at("ATE0")
        self._send_at("AT")  # sync command to make sure the following one succeeds
        self._detect_mqtt_id()
        if self._mqtt_id is not None:
            return self._mqtt_id

        if self._send_at("AT+CGREG?", "CGREG: [012],1") is None:
            logging.warning("Not registered yet")
            return None

        # Can APN be set automatically?
        response = self._send_at(
            'AT*MCGDEFCONT="IP","trial-nbiot.corp"', "OK", timeout=CONNECT_TIME
        )
        if response is None:
            logging.warning("Can not set APN")
            return None

        answers = self._send_at(
            "AT+CMQNEW?",
            f"\\+CMQNEW: [0-9],1,{BROKER_URL}",
            ["CMQNEW: ?{mqtt_id::[0-9]},1"],
            ["mqtt_id"],
        )
        if answers is not None:
            # CMQNEW is fine but CMQCON is not, the only solution is a disconnect
            self.mqtt_disconnect(int(answers[0]))

        answers = self._send_at(
            f'AT+CMQNEW="{BROKER_URL}","{BROKER_PORT}",{CONNECT_TIME}000,200',
            "CMQNEW:",
            ["CMQNEW: ?{mqtt_id::[0-9]}"],
            ["mqtt_id"],
            timeout=CONNECT_TIME + 3,
        )
        if answers is None:
            return None
        try:
            mqtt_id = int(answers[0])
            will_hex = self._will.hex()
            opt_reponse = self._send_at(
                f'AT+CMQCON={mqtt_id},3,"{self._client_name}",{CONNECT_TIME},0,1,'
                f'"topic={self._will_topic},qos=1,retained=0,'
                f'message_len={len(will_hex)},message={will_hex}"',
                "OK",
                timeout=CONNECT_TIME + 3,
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
        if (qos == 1 and time_since(self._mqtt_id_timestamp, timedelta(seconds=30))) or (
            qos == 0 and time_since(self._mqtt_id_timestamp, timedelta(minutes=3))
        ):
            if time_since(self._last_success, timedelta(minutes=8)):
                self._send_at("AT+CFUN=0", "", timeout=10)
                time.sleep(15)
                self._send_at("AT+CFUN=1", "")
                self._last_success = datetime.now()  # Do not restart too often

            if self._detect_mqtt_id() is None:
                self.mqtt_connect()

        if self._mqtt_id is None:
            logging.warning("Not connected, will not send an MQTT message")
            return False

        message_hex = message.hex()
        opt_response = self._send_at(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            "OK",
            timeout=CONNECT_TIME + 3,
        )
        success = opt_response is not None
        if success:
            self._mqtt_id_timestamp = datetime.now()
            self._last_success = datetime.now()
        return success

    def get_signal_dbm(self) -> int | None:
        answers = self._send_at("AT+CENG?", "CENG", ["CENG: ?{ceng::.*}"], ["ceng"])
        try:
            if answers is not None and len(answers) == 1:
                ceng_split = answers[0].split(",")
                return int(ceng_split[6])
            return None
        except Exception as err:
            logging.error(f"Error getting signal dBm {err}")
            return None
