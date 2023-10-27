import asyncio
import logging
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import timedelta
from typing import List

import aiohttp

from ..pb.status_pb2 import MiniCallHome


@dataclass
class MeosCategory:
    name: str
    id: str


@dataclass
class MeosCompetitor:
    name: str
    card: int | None
    bib: int | None
    id: int | None


@dataclass
class MeosResult:
    competitor: MeosCompetitor
    category: MeosCategory
    stat: int
    start: timedelta | None
    time: timedelta | None


class MopClient:
    """Class for Meos online protocol (MOP)"""

    STAT_OK = 1
    STAT_MP = 3
    STAT_DNF = 4
    STAT_OOC = 15
    STAT_DNS = 20

    def __init__(self, api_key: str, mop_xml: str | None):
        self.api_key = api_key
        if isinstance(mop_xml, str):
            self.results = MopClient.results_from_file(mop_xml)
        else:
            self.results = []

    @staticmethod
    def _parse_int(s: str | None) -> int | None:
        if s is None:
            return None
        return int(s)

    @staticmethod
    def _competitor_from_mop(cmp: ET.Element, base: ET.Element) -> MeosCompetitor:
        card, bib = MopClient._parse_int(cmp.get("card")), MopClient._parse_int(base.get("bib"))
        id = MopClient._parse_int(cmp.get("id"))
        name = "" if base.text is None else base.text
        return MeosCompetitor(name=name, card=card, bib=bib, id=id)

    @staticmethod
    def _results_from_meos_xml(xml: ET.Element) -> List[MeosResult]:
        ET.indent(xml)
        NS = {"mop": "http://www.melin.nu/mop"}
        categories = {}
        for category in xml.findall("mop:cls", NS):
            id = category.get("id")
            assert id is not None
            name = "" if category.text is None else category.text
            categories[id] = MeosCategory(name=name, id=id)

        results = []
        for cmp in xml.findall("mop:cmp", NS):
            base = cmp.find("mop:base", NS)
            if base is None:
                logging.error("No base element")
                continue
            competitor = MopClient._competitor_from_mop(cmp, base)
            stat = MopClient._parse_int(base.get("stat"))
            assert stat is not None

            st = base.get("st")
            if st is not None and st != "-1":
                start = timedelta(seconds=int(st) / 10.0)
            else:
                start = None

            rt = base.get("rt")
            if rt is not None and stat == MopClient.STAT_OK:
                total_time = timedelta(seconds=int(rt) / 10.0)
            else:
                total_time = None
            cat_id = base.get("cls")
            assert cat_id is not None
            results.append(
                MeosResult(
                    competitor=competitor,
                    category=categories[cat_id],
                    stat=stat,
                    time=total_time,
                    start=start,
                )
            )
        return results

    @staticmethod
    def _competitors_from_meos_xml(xml: ET.Element) -> List[MeosCompetitor]:
        ET.indent(xml)
        NS = {"mop": "http://www.melin.nu/mop"}
        competitors = []
        for cmp in xml.findall("mop:cmp", NS):
            base = cmp.find("mop:base", NS)
            if base is None:
                logging.error("No base element")
                continue
            competitors.append(MopClient._competitor_from_mop(cmp, base))
        return competitors

    @staticmethod
    def _result_to_xml(result: MeosResult) -> ET.Element:
        competitor = result.competitor
        root = ET.Element("cmp", {"card": str(result.competitor.card), "id": str(competitor.id)})
        st = "-1" if result.start is None else str(result.start.seconds * 10)
        rt = "0" if result.time is None else str(result.time.seconds * 10)
        cls = str(result.category.id)
        base = ET.Element(
            "base",
            {"org": "22", "st": st, "rt": rt, "cls": cls, "stat": str(result.stat)},
        )
        base.text = competitor.name
        root.append(base)
        return root

    async def loop(self):
        timeout = aiohttp.ClientTimeout(total=20)
        self.session = aiohttp.ClientSession(timeout)
        async with self.session:
            await asyncio.sleep(1000000)

    @staticmethod
    def results_from_file(filename: str) -> List[MeosResult]:
        xml = ET.parse(filename)
        return MopClient._results_from_meos_xml(xml.getroot())

    async def send_result(self, result: MeosResult):
        root = ET.Element("MOPDiff", {"xmlns": "http://www.melin.nu/mop"})
        root.append(MopClient._result_to_xml(result))
        headers = {"pwd": self.api_key}

        async with self.session.post(
            "https://api.oresults.eu/meos", data=ET.tostring(root), headers=headers
        ) as response:
            if response.status == 200:
                logging.info("Sending to OResults successful")
                logging.debug("Response: {} {}", response, await response.text())
            else:
                logging.error("Sending unsuccessful: {} {}", response, await response.text())

    async def fetch_results(self, address: str, port: int) -> List[MeosResult]:
        async with self.session.get(f"http://{address}:{port}/meos?difference=zero") as response:
            assert response.status == 200
            xml = ET.XML(response.text)

            return MopClient._results_from_meos_xml(xml)

    async def competitors(self, address: str, port: int) -> List[MeosCompetitor]:
        async with self.session.get(f"http://{address}:{port}/meos?difference=zero") as response:
            assert response.status == 200
            xml = ET.XML(response.text)

            return MopClient._competitors_from_meos_xml(xml)

    async def send_mini_call_home(self, mch: MiniCallHome) -> bool:
        return True
