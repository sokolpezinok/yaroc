import time
import unittest
from datetime import datetime, timedelta

from yaroc.utils.retries import BackoffRetries, BatchRetries


class TestRetries(unittest.TestCase):
    def test_backoff_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {2: 0, 4: 0}

        def send_f(x: int) -> datetime:
            stats[x] += 1
            if stats[x] < x:
                raise Exception(f"Failed arg={x} for the {stats[x]}th time")
            return datetime.now()

        b = BackoffRetries(send_f, lambda x: x, 0.04, 2.0, timedelta(minutes=0.1))
        start = datetime.now()
        f2 = b.send(2)
        time.sleep(0.13)
        f4 = b.send(4)
        finished2 = f2.result()
        finished4 = f4.result()
        published4 = datetime.now()
        b.close(0.2)

        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.04)).timestamp(),
            delta=0.004,
        )
        self.assertAlmostEqual(
            finished4.timestamp(),
            (start + timedelta(seconds=0.41)).timestamp(),
            delta=0.004,
        )
        self.assertAlmostEqual(finished4.timestamp(), published4.timestamp(), delta=0.004)

    def test_batch_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        failures = {1: 2, 2: 3, 3: 0}

        def send_f(messages: list[int]):
            one_failed = False
            for message in messages:
                time.sleep(0.1)
                if failures[message] > 0:
                    one_failed = True
                failures[message] -= 1
            if one_failed:
                raise Exception("W")

        b = BatchRetries(send_f, 2)
        start = datetime.now()
        z1 = b.send(1)
        time.sleep(0.05)
        z2 = b.send(2)
        time.sleep(0.05)
        z3 = b.send(3)
        z1.wait_for_publish()
        finished1 = datetime.now()
        z3.wait_for_publish()
        finished3 = datetime.now()
        z2.wait_for_publish()
        finished2 = datetime.now()

        self.assertAlmostEqual(
            finished1.timestamp(),
            finished3.timestamp(),
            delta=0.005,
        )
        self.assertAlmostEqual(
            finished1.timestamp(),
            (start + timedelta(seconds=0.7)).timestamp(),
            delta=0.02,
        )
        self.assertAlmostEqual(
            finished2.timestamp(),
            (start + timedelta(seconds=0.9)).timestamp(),
            delta=0.02,
        )


if __name__ == "__main__":
    unittest.main()
