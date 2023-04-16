FROM docker.io/python:3.11-slim-bullseye

COPY . .
RUN pip install .
