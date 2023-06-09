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
class MeosCompetitor:
    name: str
    card: int | None
    bib: int | None


@dataclass
class MeosResult:
    competitor: MeosCompetitor
    category: MeosCategory
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
    def _parse_int(s: str | None) -> int | None:
        if s is None:
            return None
        return int(s)

    @staticmethod
    def _competitor_from_mop(cmp: ET.Element, base: ET.Element) -> MeosCompetitor:
        card, bib = MOP._parse_int(cmp.get("card")), MOP._parse_int(base.get("bib"))
        name = "" if base.text is None else base.text
        return MeosCompetitor(name=name, card=card, bib=bib)

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
            competitor = MOP._competitor_from_mop(cmp, base)
            stat = MOP._parse_int(base.get("stat"))
            assert stat is not None

            rt = base.get("rt")
            if rt is not None and stat == MOP.STAT_OK:
                total_time = timedelta(seconds=int(rt) / 10.0)
            else:
                total_time = None
            cat_id = base.get("cls")
            assert cat_id is not None
            results.append(
                MeosResult(
                    competitor=competitor, category=categories[cat_id], stat=stat, time=total_time
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
            competitors.append(MOP._competitor_from_mop(cmp, base))
        return competitors

    @staticmethod
    def results(address: str, port: int) -> List[MeosResult]:
        response = requests.get(
            f"http://{address}:{port}/meos?difference=zero",
        )
        assert response.status_code == 200
        xml = ET.XML(response.text)

        return MOP._results_from_meos_xml(xml)

    @staticmethod
    def competitors(address: str, port: int) -> List[MeosCompetitor]:
        response = requests.get(
            f"http://{address}:{port}/meos?difference=zero",  # TODO: it could be asimpler call
        )
        assert response.status_code == 200
        xml = ET.XML(response.text)

        return MOP._competitors_from_meos_xml(xml)
