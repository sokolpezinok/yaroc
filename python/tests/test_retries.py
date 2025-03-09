import asyncio
import unittest
from datetime import datetime, timedelta

import pytest

from yaroc.utils.retries import BackoffBatchedRetries


@pytest.mark.skip(reason="Fails too often in CI")
class TestBatchedBackoffRetries(unittest.IsolatedAsyncioTestCase):
    async def test_backoff_batched_retries(self):
        # Note: this is not the best test as it is non-deterministic, but the error-margin is
        # pretty wide
        stats = {1: 0, 2: 0, 3: 0}

        async def send_f(xs: list[int]) -> list[datetime | None]:
            ret: list[datetime | None] = []
            for x in xs:
                await asyncio.sleep(0.04)
                if stats[x] < x:
                    stats[x] += 1
                    ret.append(None)
                else:
                    ret.append(datetime.now())
            return ret

        b = BackoffBatchedRetries(send_f, None, 0.03, 2.0, timedelta(seconds=10), batch_count=2)

        async def sleep_and_1():
            await asyncio.sleep(0.004)
            return await b.send(1)

        async def sleep_and_2():
            await asyncio.sleep(0.002)
            return await b.send(2)

        start = datetime.now()
        [finished1, finished2, finished3] = await asyncio.gather(
            sleep_and_1(), sleep_and_2(), b.send(3)
        )

        self.assertAlmostEqual(
            (finished1 - start).microseconds,
            242_000,
            delta=20_000,
        )
        self.assertAlmostEqual(
            (finished2 - start).microseconds,
            342_000,
            delta=20_000,
        )
        self.assertAlmostEqual(
            (finished3 - start).microseconds,
            445_000,
            delta=20_000,
        )


if __name__ == "__main__":
    unittest.main()
