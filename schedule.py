#!/usr/bin/env python3
import logging
import math
import sched
import time
from collections.abc import Callable
from datetime import datetime, timedelta
from typing import Any, Optional, TypeVar, Generic

FIRST_BACKOFF = 1.0
PRIORITY = 1


class UnsentMessage:
    def __init__(self, argument: tuple[Any], deadline: datetime, backoff: float):
        self.argument = argument
        self.deadline = deadline
        self.backoff = backoff

    def new_backoff(self, multiplier: float):
        self.backoff *= multiplier


T = TypeVar("T")


class BackoffSender(Generic[T]):
    """
    A sender that does exponential backoff in case of failed send operations

    The class is thread-safe
    """
    def __init__(
        self,
        send_function: Callable[[Any], T],
        max_duration: timedelta,
        multiplier: float,
    ):
        self.scheduler = sched.scheduler(time.time)
        self.send_function = send_function
        self.max_duration = max_duration
        self.multiplier = multiplier

    def send(self, argument: tuple[Any]):
        def wrapped_function(unsent_message: UnsentMessage) -> Optional[T]:
            try:
                return self.send_function(*unsent_message.argument)
            except Exception as e:
                logging.error(e)
                cur_backoff = unsent_message.backoff
                unsent_message.new_backoff(self.multiplier)
                if (
                    datetime.now() + timedelta(seconds=cur_backoff)
                    < unsent_message.deadline
                ):
                    logging.info(f"Retrying after {cur_backoff} seconds")
                    self.scheduler.enter(
                        cur_backoff,
                        PRIORITY,
                        wrapped_function,
                        (unsent_message,),
                    )
                else:
                    logging.info(f"Message expired, args = {unsent_message.argument}")

        self.scheduler.enter(
            0.0,
            PRIORITY,
            wrapped_function,
            kwargs={
                "unsent_message": UnsentMessage(
                    argument, datetime.now() + self.max_duration, FIRST_BACKOFF
                )
            },
        )
        logging.debug("Scheduled")

    def loop(self):
        # Schedule a job far away, so that the scheduler loops "forever"
        self.scheduler.enter(
            timedelta(days=7).total_seconds(), PRIORITY, (lambda: 0), ()
        )
        while True:
            self.scheduler.run()


logging.basicConfig(
    encoding="utf-8",
    level=logging.DEBUG,
    format="%(asctime)s - %(levelname)s - %(message)s",
)


stats = {1: 0, 2: 0, 3: 0, 4: 0}


def f(x: int):
    stats[x] += 1
    if stats[x] == x:
        logging.info(x + 1)
    else:
        raise Exception(f"Failed arg={x} for the {stats[x]}th time")


b = BackoffSender(f, timedelta(minutes=0.3), math.sqrt(2.0))
b.send(argument=(2,))
time.sleep(0.5)
b.send(argument=(4,))
b.loop()
