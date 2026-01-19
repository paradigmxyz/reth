FROM python:3.12-slim

WORKDIR /app

# Install dependencies (extracted from inline script metadata)
RUN pip install --no-cache-dir fastapi uvicorn httpx cachetools

COPY app.py .

EXPOSE 5050

ENTRYPOINT ["python", "app.py"]
CMD ["--port", "5050"]
