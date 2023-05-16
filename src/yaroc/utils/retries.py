import asyncio
import logging
from asyncio import Condition
from collections.abc import Callable
from concurrent.futures import Future, ThreadPoolExecutor
from datetime import datetime, time, timedelta
from queue import Queue
from threading import Thread
from typing import Any, Generic, Optional, Tuple, TypeVar

T = TypeVar("T")
A = TypeVar("A")


class BackoffRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[A], T],
        on_publish: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
        workers: int = 1,
    ):
        self.send_function = send_function
        self.on_publish = on_publish
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.executor = ThreadPoolExecutor(max_workers=workers)

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        self._loop = asyncio.new_event_loop()
        self._thread = Thread(target=start_background_loop, args=(self._loop,), daemon=True)
        self._thread.start()

    async def _backoff_send(self, argument: A) -> Optional[T]:
        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff

        while datetime.now() < deadline:
            try:
                ret = await self._loop.run_in_executor(self.executor, self.send_function, argument)
                try:
                    self.on_publish(argument)
                finally:
                    return ret
            except Exception as e:
                logging.error(f"Caught exception while sending: {e}")
                if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                    cur_backoff = (deadline - datetime.now()).total_seconds()
                logging.info(f"Retrying after {cur_backoff} seconds")
                await asyncio.sleep(cur_backoff)
                cur_backoff = cur_backoff * self.multiplier

        logging.info(f"Message expired, args = {argument}")
        return None

    def close(self, timeout=None):
        self._thread.join(timeout)

    def send(self, argument: A) -> Future:
        future = asyncio.run_coroutine_threadsafe(self._backoff_send(argument), self._loop)
        logging.debug("Scheduled")  # TODO: add message ID
        return future

    def execute(self, fn, *args) -> Future:
        return self.executor.submit(fn, *args)


class RetriedMessage(Generic[A, T]):
    def __init__(self, arg: A):
        self.processed = Condition()
        self.published = False
        self.returned: T | None = None
        self.arg = arg

    async def set_published(self, returned: T):
        async with self.processed:
            self.published = True
            self.returned = returned
            self.processed.notify()

    async def set_not_published(self):
        async with self.processed:
            self.processed.notify()

    def wait_for_publish(self, timeout=None):
        timeout_time = None if timeout is None else time.time() + timeout
        timeout_tenth = None if timeout is None else timeout / 10.0

        def timed_out():
            return False if timeout_time is None else time.time() > timeout_time

        while not timed_out():
            self.processed.wait(timeout_tenth)


class BackoffBatchedRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[list[A]], list[T]],
        on_publish: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
        batch_count: int = 2,
        workers: int = 1,
    ):
        self.send_function = send_function
        self.on_publish = on_publish
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.batch_count = batch_count
        self._executor = ThreadPoolExecutor(max_workers=workers)
        self._queue: Queue[RetriedMessage] = Queue()

        def start_background_loop(loop: asyncio.AbstractEventLoop) -> None:
            asyncio.set_event_loop(loop)
            loop.run_forever()

        self._loop = asyncio.new_event_loop()
        self._thread = Thread(target=start_background_loop, args=(self._loop,), daemon=True)
        self._thread.start()

    def _send_queued(self) -> Tuple[list[RetriedMessage], Exception | list[T]]:
        messages = []
        while not self._queue.empty():
            message = self._queue.get()
            messages.append(message)
            if len(messages) >= self.batch_count:
                break
        if len(messages) == 0:
            return ([], [])
        try:
            returned = self.send_function([message.arg for message in messages])
            return (messages, returned)
        except Exception as err:
            return (messages, err)

    async def _send_and_notify(self):
        (messages, returned) = await self._loop.run_in_executor(
            self._executor, self._send_queued
        )
        if isinstance(returned, list):
            for message, r in zip(messages, returned):
                await message.set_published(r)
        else:
            logging.error(returned)
            for message in messages:
                await message.set_not_published()

    async def _backoff_send(self, argument: A) -> Optional[T]:
        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff
        retried_message: RetriedMessage[A, T] = RetriedMessage(argument)

        while datetime.now() < deadline:
            async with retried_message.processed:
                self._queue.put(retried_message)
                asyncio.run_coroutine_threadsafe(self._send_and_notify(), self._loop)
                await retried_message.processed.wait()
                if retried_message.published:
                    return retried_message.returned

            if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                cur_backoff = (deadline - datetime.now()).total_seconds()
                if cur_backoff < 0:
                    break
            logging.info(f"Retrying after {cur_backoff} seconds")
            await asyncio.sleep(cur_backoff)
            cur_backoff = cur_backoff * self.multiplier

        logging.info(f"Message expired, args = {argument}")
        return None

    def close(self, timeout=None):
        self._thread.join(timeout)

    def send(self, argument: A) -> Future:
        future = asyncio.run_coroutine_threadsafe(self._backoff_send(argument), self._loop)
        logging.debug("Scheduled")  # TODO: add message ID
        return future

    def execute(self, fn, *args) -> Future:
        return self._executor.submit(fn, *args)
