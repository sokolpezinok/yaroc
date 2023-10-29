import asyncio
import logging
from asyncio import Condition, Lock
from collections.abc import Awaitable, Callable
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta
from queue import Queue
from typing import Generic, Optional, Tuple, TypeVar

T = TypeVar("T")
A = TypeVar("A")


class BackoffRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[A], Awaitable[T]],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.send_function = send_function
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self._current_mid = 0

    async def backoff_send(self, argument: A) -> Optional[T]:
        self._current_mid += 1
        mid = self._current_mid
        logging.debug(f"Scheduled: {mid}")

        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff
        while datetime.now() < deadline:
            try:
                ret = await self.send_function(argument)
                if ret is not None:
                    logging.info(f"Punch sent: {mid}")
                    return ret
            except Exception as err:
                logging.error(f"Sending failed: {err}")

            if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                cur_backoff = (deadline - datetime.now()).total_seconds()
            if cur_backoff < 0:
                break
            logging.error(f"Punch not sent: {mid}, retrying after {cur_backoff} seconds")
            await asyncio.sleep(cur_backoff)
            cur_backoff = cur_backoff * self.multiplier

        logging.error(f"Message mid={mid} expired, args = {argument}")
        return None


class RetriedMessage(Generic[A, T]):
    def __init__(self, arg: A, mid: int):
        self.processed = Condition()
        self.published = False
        self.returned: T | None = None
        self.mid = mid
        self.arg = arg

    async def set_published(self, returned: T):
        async with self.processed:
            self.published = True
            self.returned = returned
            self.processed.notify()

    async def set_not_published(self):
        async with self.processed:
            self.processed.notify()


class BackoffBatchedRetries(Generic[A, T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """

    def __init__(
        self,
        send_function: Callable[[list[A]], list[T | None]],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
        batch_count: int = 2,
        workers: int = 1,
    ):
        self.send_function = send_function
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier
        self.batch_count = batch_count
        self._executor = ThreadPoolExecutor(max_workers=workers)
        self._queue: Queue[RetriedMessage] = Queue()
        self._current_mid_lock = Lock()
        self._current_mid = 0

    def _send_queued(self) -> Tuple[list[RetriedMessage], list[T | None]]:
        messages = []
        while not self._queue.empty():
            message = self._queue.get()
            messages.append(message)
            if len(messages) >= self.batch_count:
                break
        if len(messages) == 0:
            return ([], [])

        returned = self.send_function([message.arg for message in messages])
        return (messages, returned)

    async def _send_and_notify(self):
        (messages, returned) = await asyncio.get_event_loop().run_in_executor(
            self._executor, self._send_queued
        )
        published, not_published = [], []
        for message, r in zip(messages, returned):
            if r is None:
                await message.set_not_published()
                not_published.append(message.mid)
            else:
                await message.set_published(r)
                published.append(message.mid)

        if len(published) > 0:
            logging.info("Messages sent: " + ",".join(map(str, published)))
        if len(not_published) > 0:
            logging.error("Messages not sent: " + ",".join(map(str, not_published)))

    async def _backoff_send(self, argument: A) -> Optional[T]:
        async with self._current_mid_lock:
            self._current_mid += 1
            retried_message: RetriedMessage[A, T] = RetriedMessage(argument, self._current_mid)
        logging.debug(f"Scheduled: mid={retried_message.mid}")

        deadline = datetime.now() + self.max_duration
        cur_backoff = self.first_backoff
        while datetime.now() < deadline:
            async with retried_message.processed:
                self._queue.put(retried_message)
                asyncio.run_coroutine_threadsafe(self._send_and_notify(), asyncio.get_event_loop())
                await retried_message.processed.wait()
                if retried_message.published:
                    return retried_message.returned

            if datetime.now() + timedelta(seconds=cur_backoff) >= deadline:
                cur_backoff = (deadline - datetime.now()).total_seconds()
                if cur_backoff < 0:
                    break
            logging.info(f"Retrying mid={retried_message.mid} after {cur_backoff} seconds")
            await asyncio.sleep(cur_backoff)
            cur_backoff = cur_backoff * self.multiplier

        logging.error(f"Message mid={retried_message.mid} expired, args = {argument}")
        return None

    async def send(self, argument: A) -> Optional[T]:
        return await self._backoff_send(argument)

    async def execute(self, fn, *args):
        return await asyncio.get_event_loop().run_in_executor(self._executor, fn, *args)
