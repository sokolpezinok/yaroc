import logging

from attila.atcommand import ATCommand
from attila.atre import ATRuntimeEnvironment
from attila.exceptions import ATRuntimeError, ATScriptSyntaxError, ATSerialPortError

# TODO: either share these constants or make them parameters
BROKER_URL = "broker.hivemq.com"
BROKER_PORT = 1883


class SIM7020Interface:
    """An AT interface to the SIM7020 NB-IoT chip

    Implements mostly MQTT functionality

    Note: this class is not thread-safe.
    """

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
        if mqtt_id is not None:
            self.mqtt_disconnect(mqtt_id)

    def mqtt_disconnect(self, mqtt_id: int | None):
        if mqtt_id is not None:
            self._send_at(f"AT+CMQDISCON={mqtt_id}", "OK")

    def _detect_mqtt_id(self) -> int | None:
        try:
            answers = self._send_at(
                "AT+CMQCON?",
                f'CMQCON: [0-9],1,"{BROKER_URL}"',
                ["CMQCON: ?{id::[0-9]},1"],
                ["id"],
            )
            if answers is not None and len(answers) == 1:
                return int(answers[0])
            return None
        except Exception:
            return None

    def mqtt_connect(self) -> int | None:
        self._mqtt_id = self._mqtt_connect_internal()
        return self._mqtt_id

    def _mqtt_connect_internal(self) -> int | None:
        """Should only work under a global lock"""
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

        answers = self._send_at(
            "AT+CMQNEW?",
            f"\\+CMQNEW: [0-9],1,{BROKER_URL}",
            ["CMQNEW: ?{mqtt_id::[0-9]},1"],
            ["mqtt_id"],
        )
        if answers is not None:
            self.mqtt_disconnect(int(answers[0]))

        answers = self._send_at(
            f'AT+CMQNEW="{BROKER_URL}","{BROKER_PORT}",60000,100',
            "CMQNEW:",
            ["CMQNEW: ?{mqtt_id::[0-9]}"],
            ["mqtt_id"],
            timeout=35,
        )
        if answers is None:
            return None
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
                self._mqtt_id = self.mqtt_connect()

        message_hex = message.hex()
        opt_response = self._send_at(
            f'AT+CMQPUB={self._mqtt_id},"{topic}",{qos},0,0,{len(message_hex)},"{message_hex}"',
            "OK",
            timeout=60,
        )
        return opt_response is not None

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
