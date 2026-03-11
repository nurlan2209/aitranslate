"""
Download the Vosk Kazakh model automatically.
Run this script once before starting the server:
    python download_model.py
"""
import os
import zipfile
import urllib.request

MODEL_URL = "https://alphacephei.com/vosk/models/vosk-model-small-kz-0.15.zip"
MODEL_DIR = "vosk-model-small-kz-0.15"
ZIP_FILE = "vosk-model-small-kz-0.15.zip"

def download_model():
    if os.path.isdir(MODEL_DIR):
        print(f"✅ Model '{MODEL_DIR}' already exists. Skipping download.")
        return

    print(f"⬇️  Downloading Kazakh Vosk model (~50 MB)...")
    print(f"   URL: {MODEL_URL}")
    
    urllib.request.urlretrieve(MODEL_URL, ZIP_FILE)
    print("📦 Extracting...")
    
    with zipfile.ZipFile(ZIP_FILE, 'r') as zip_ref:
        zip_ref.extractall(".")
    
    os.remove(ZIP_FILE)
    print(f"✅ Model extracted to '{MODEL_DIR}'")

if __name__ == "__main__":
    download_model()
