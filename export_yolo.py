"""
Export a YOLOv8 table-detection model to ONNX format for BrainPipe-Nano.
Uses foduucom/table-detection-and-extraction from HuggingFace Hub.
"""
import os
import shutil
from huggingface_hub import hf_hub_download
from ultralytics import YOLO

os.makedirs('models', exist_ok=True)

print("[1/3] Downloading model from HuggingFace Hub...")
model_path = hf_hub_download(
    repo_id="foduucom/table-detection-and-extraction",
    filename="best.pt",
)
print(f"      Downloaded to: {model_path}")

print("[2/3] Loading model with Ultralytics...")
model = YOLO(model_path)

# Print model info for debugging
print(f"      Model task: {model.task}")
print(f"      Model names: {model.names}")

print("[3/3] Exporting to ONNX (imgsz=640)...")
export_path = model.export(format='onnx', imgsz=640, simplify=True)
print(f"      Exported to: {export_path}")

# Copy to our models directory
dst = os.path.join('models', 'nano_layout.onnx')
shutil.copy(export_path, dst)
print(f"\n✅ Model ready at: {dst}")
print(f"   File size: {os.path.getsize(dst) / 1024 / 1024:.1f} MB")
