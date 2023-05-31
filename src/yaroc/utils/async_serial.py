import asyncio
import logging
import re
from dataclasses import dataclass
from datetime import datetime
from threading import Thread
from typing import Callable, Dict

from serial_asyncio import open_serial_connection


@dataclass
class ATResponse:
    full_response: list[str]
    query: list[str] | None = None
    success: bool = False


class AsyncATCom:
    def __init__(self, port: str):
        self.callbacks: Dict[str, Callable[[str], None]] = {}
        self.add_callback("+CLTS", lambda x: None)
        self.add_callback("+CPIN", lambda x: None)
        # self.add_callback("+CGREG")

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        self._loop = asyncio.new_event_loop()
        self._thread = Thread(target=start_background_loop, args=(self._loop,), daemon=True)
        self._thread.start()
        self.port = port
        asyncio.run_coroutine_threadsafe(self._open_port(), self._loop).result()

        self._lock = asyncio.Lock()

    def add_callback(self, prefix: str, fn: Callable[[str], None]):
        self.callbacks[prefix] = fn

    async def _open_port(self):
        reader, writer = await open_serial_connection(url=self.port, baudrate=115200, rtscts=False)
        self.reader = reader
        self.writer = writer

    def match_callback(self, line: str) -> Callable[[str], None] | None:
        for prefix, callback in self.callbacks.items():
            if line.startswith(prefix):
                return callback
        return None

    async def _call_until_with_timeout(
        self, command: str, timeout: float = 60, last_line: str = "OK|ERROR"
    ) -> list[str]:
        async with self._lock:
            try:
                async with asyncio.timeout(timeout):
                    return await self._call_until(command, last_line)
            except asyncio.TimeoutError:
                logging.error(f"Timed out: {command}")
                return []

    async def _call_until(self, command: str, last_line: str = "OK|ERROR") -> list[str]:
        """Call until 'last_line' matches"""
        self.writer.write((command + "\r\n").encode("utf-8"))
        regex = re.compile(last_line)
        full_response: list[str] = []
        while True:
            line = (await self.reader.readline()).strip().decode("utf-8")
            if len(line) == 0:
                continue  # Skip empty lines

            callback = self.match_callback(line)
            if callback is not None:
                callback(line)
                continue

            full_response.append(line)
            if regex.match(line):
                return full_response

    def call(
        self,
        command: str,
        match: str | None = None,
        field_no: int | None = None,
        timeout: float = 60,
    ) -> ATResponse:
        full_response = asyncio.run_coroutine_threadsafe(
            self._call_until_with_timeout(command, timeout), self._loop
        ).result()
        res = ATResponse(full_response)
        logging.debug(f"{command} {full_response}")
        if len(res.full_response) == 0 or res.full_response[-1] == "ERROR":
            return res
        if match is None:
            res.success = True
            return res

        regex = re.compile(match)
        for line in res.full_response:
            found = regex.search(line)
            if found is None:
                continue

            res.query = []
            res.success = True
            for group in found.groups():
                assert type(group) == str
                if field_no is not None:
                    res.query = [group.split(",")[field_no]]
                else:
                    res.query.append(group)
            return res
        return res
