import brainpipe
import os

# 1. Create a dummy data directory if it doesn't exist
DATA_DIR = "./example_data"
if not os.path.exists(DATA_DIR):
    os.makedirs(DATA_DIR)
    with open(os.path.join(DATA_DIR, "hello.txt"), "w") as f:
        f.write("Hello from BrainPipe!\n\nThis is a simple text file to demonstrate semantic chunking.\n\nEverything is handled in Rust.")

print(f"--- BrainPipe Quickstart ---")

# 2. Ingest the directory
# This handles .pdf, .txt, and .md files automatically
try:
    docs = brainpipe.ingest(DATA_DIR)
    
    print(f"Successfully ingested {len(docs)} documents.\n")

    for doc in docs:
        print(f"Document: {os.path.basename(doc.path)}")
        print(f"Chunks:   {doc.num_chunks}")
        print(f"Preview:  {doc.content[:50]}...")
        print("-" * 30)

except Exception as e:
    print(f"An error occurred: {e}")
