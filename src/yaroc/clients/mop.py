import logging
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from datetime import timedelta
from typing import List

import requests


@dataclass
class MeosCategory:
    name: str
    id: str


@dataclass
class MeosResult:
    category: MeosCategory
    name: str
    card: int | None
    stat: int
    time: timedelta | None


class MOP:
    """Class for Meos online protocol (MOP)"""

    STAT_OK = 1
    STAT_MP = 3
    STAT_DNF = 4
    STAT_OOC = 15
    STAT_DNS = 20

    @staticmethod
    def _meos_results_xml(xml: ET.Element) -> List[MeosResult]:
        def parse_int(s: str | None) -> int | None:
            if s is None:
                return None
            return int(s)

        ET.indent(xml)
        NS = {"mop": "http://www.melin.nu/mop"}
        categories = {}
        for category in xml.findall("mop:cls", NS):
            id = category.get("id")
            assert id is not None
            name = "" if category.text is None else category.text
            categories[id] = MeosCategory(name=name, id=id)

        results = []
        for result in xml.findall("mop:cmp", NS):
            base = result.find("mop:base", NS)
            if base is None:
                logging.error(f"No base element")
                continue
            card, stat = parse_int(result.get("card")), parse_int(base.get("stat"))
            assert stat is not None
            name = "" if base.text is None else base.text

            rt = base.get("rt")
            if rt is not None and stat == MOP.STAT_OK:
                total_time = timedelta(seconds=int(rt) / 10.0)
            else:
                total_time = None
            cat_id = base.get("cls")
            assert cat_id is not None
            results.append(
                MeosResult(
                    name=name, card=card, stat=stat, category=categories[cat_id], time=total_time
                )
            )
        return results

    @staticmethod
    def meos_results(address: str, port: int) -> List[MeosResult]:
        response = requests.get(
            f"http://{address}:{port}/meos?difference=zero",
        )
        assert response.status_code == 200
        xml = ET.XML(response.text)

        return MOP._meos_results_xml(xml)