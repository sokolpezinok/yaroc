import asyncio
import itertools
import logging
import re
from asyncio import StreamReader, StreamWriter
from dataclasses import dataclass
from typing import Callable, Dict, Tuple

from serial_asyncio import open_serial_connection


@dataclass
class ATResponse:
    full_response: list[str]
    query: list[str] | None = None
    success: bool = False


class AsyncATCom:
    def __init__(
        self, reader: StreamReader, writer: StreamWriter, async_loop: asyncio.AbstractEventLoop
    ):
        self.callbacks: Dict[str, Callable[[str], None]] = {}
        self.add_callback("+CLTS", lambda x: None)
        self.add_callback("+CPIN", lambda x: None)
        self.delay = 0.05  # TODO: make configurable

        self._lock = asyncio.Lock()
        self._reader = reader
        self._writer = writer
        self._loop = async_loop

    @staticmethod
    def atcom_from_port(port: str, async_loop: asyncio.AbstractEventLoop):
        async def open_port(port: str) -> Tuple[StreamReader, StreamWriter]:
            return await open_serial_connection(url=port, baudrate=115200, rtscts=False)

        reader, writer = asyncio.run_coroutine_threadsafe(open_port(port), async_loop).result()
        return AsyncATCom(reader, writer, async_loop)

    def add_callback(self, prefix: str, fn: Callable[[str], None]):
        self.callbacks[prefix] = fn

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
        pre_read = []
        try:
            async with asyncio.timeout(self.delay):
                while True:
                    line = (await self._reader.readline()).strip().decode("utf-8")
                    if len(line) == 0:
                        continue  # Skip empty lines
                    pre_read.append(line)
        except asyncio.TimeoutError:
            if len(pre_read) > 0:
                logging.info(f"Read {pre_read} at the start")

        self._writer.write((command + "\r\n").encode("utf-8"))
        regex = re.compile(last_line)
        full_response: list[str] = []
        while True:
            line = (await self._reader.readline()).strip().decode("utf-8")
            if len(line) == 0:
                continue  # Skip empty lines

            full_response.append(line)
            if regex.match(line):
                break

        for line in itertools.chain(pre_read, full_response):
            callback = self.match_callback(line)
            if callback is not None:
                callback(line)
                continue

        return full_response

    def call(
        self,
        command: str,
        match: str | None = None,
        field_no: int | None = None,
        timeout: float = 20,
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
