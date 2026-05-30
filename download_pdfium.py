import urllib.request
import tarfile
import os

url = "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-win-x64.tgz"
tar_path = "pdfium.tgz"

print("Downloading Pdfium...")
urllib.request.urlretrieve(url, tar_path)

print("Extracting...")
with tarfile.open(tar_path, "r:gz") as tar:
    tar.extractall()

if os.path.exists("pdfium.dll"):
    os.remove("pdfium.dll")
os.rename("bin/pdfium.dll", "pdfium.dll")

os.remove(tar_path)
print("Pdfium downloaded and extracted.")
