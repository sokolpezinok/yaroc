import logging
import sched
import time
from collections.abc import Callable
from datetime import datetime, timedelta
from typing import Any, Generic, Optional, TypeVar

PRIORITY = 1


class UnsentMessage:
    # TODO: make backoff a timedelta
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
        callback: Callable[[Any], Any],
        first_backoff: float,
        multiplier: float,
        max_duration: timedelta,
    ):
        self.scheduler = sched.scheduler(time.time)
        self.send_function = send_function
        self.callback = callback
        self.first_backoff = first_backoff
        self.max_duration = max_duration
        self.multiplier = multiplier

    def send(self, argument: tuple[Any]):
        def wrapped_function(unsent_message: UnsentMessage) -> Optional[T]:
            try:
                ret = self.send_function(*unsent_message.argument)
                self.callback(*unsent_message.argument)
                return ret
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
                    self.scheduler.run(False)
                else:
                    logging.info(f"Message expired, args = {unsent_message.argument}")

        self.scheduler.enter(
            0.0,
            PRIORITY,
            wrapped_function,
            kwargs={
                "unsent_message": UnsentMessage(
                    argument, datetime.now() + self.max_duration, self.first_backoff
                )
            },
        )
        logging.debug("Scheduled")
        self.scheduler.run(False)
