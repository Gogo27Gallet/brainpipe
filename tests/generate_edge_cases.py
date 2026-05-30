import os
from reportlab.pdfgen import canvas
from reportlab.lib.pagesizes import letter

TEST_DATA_DIR = "./tests/edge_cases"

def ensure_dir():
    if not os.path.exists(TEST_DATA_DIR):
        os.makedirs(TEST_DATA_DIR)

def gen_multi_column():
    path = os.path.join(TEST_DATA_DIR, "multi_column.pdf")
    c = canvas.Canvas(path, pagesize=letter)
    c.drawString(50, 750, "Multi-Column Layout Test")
    
    # Simulate two columns
    text_col1 = "Column One " * 20
    text_col2 = "Column Two " * 20
    
    for i in range(30):
        c.drawString(50, 700 - (i*20), text_col1)
        c.drawString(300, 700 - (i*20), text_col2)
    
    c.save()
    print(f"Generated: {path}")

def gen_corrupted_encoding():
    # We simulate a PDF where the text layer might be weird
    # Here we just put some non-latin characters that often break weak parsers
    path = os.path.join(TEST_DATA_DIR, "unicode_chaos.pdf")
    c = canvas.Canvas(path, pagesize=letter)
    c.drawString(50, 750, "Unicode & Encoding Chaos")
    
    weird_text = "Mtr d'ingestion.   . 你好. Привет. 🛠️⚡"
    for i in range(10):
        c.drawString(50, 700 - (i*20), weird_text)
    
    c.save()
    print(f"Generated: {path}")

def gen_massive_single_page():
    path = os.path.join(TEST_DATA_DIR, "massive_page.pdf")
    c = canvas.Canvas(path, pagesize=letter)
    c.drawString(50, 750, "Massive Content Single Page")
    
    # Put thousands of small strings on one page
    for i in range(100):
        c.drawString(50, 730 - (i*7), "Small line of text " * 10)
    
    c.save()
    print(f"Generated: {path}")

if __name__ == "__main__":
    ensure_dir()
    gen_multi_column()
    gen_corrupted_encoding()
    gen_massive_single_page()
