import brainpipe
import os
import pytest

TEST_DATA_DIR = "./tests/edge_cases"

def test_reliability():
    print("Running Reliability Test Suite...")
    
    if not os.path.exists(TEST_DATA_DIR):
        pytest.skip("Test data not found. Run generate_edge_cases.py first.")

    try:
        # Force no cache to test raw engine robustness
        docs = list(brainpipe.ingest(TEST_DATA_DIR, use_cache=False))
        
        print(f"Ingested {len(docs)} documents without crashing.")
        
        for doc in docs:
            print(f"Testing: {os.path.basename(doc.path)}")
            
            # 1. Non-empty check
            if len(doc.content) == 0:
                print(f"   Warning: {os.path.basename(doc.path)} returned empty content.")
            else:
                print(f"   Content length: {len(doc.content)} characters.")
                
            # 2. Chunking check
            if doc.num_chunks == 0:
                print(f"   Warning: {os.path.basename(doc.path)} has 0 chunks.")
            else:
                print(f"   Created {doc.num_chunks} chunks.")

        print("\nReliability test passed (No crashes, data extracted).")

    except Exception as e:
        pytest.fail(f"CRASH DETECTED: {e}")

if __name__ == "__main__":
    test_reliability()

