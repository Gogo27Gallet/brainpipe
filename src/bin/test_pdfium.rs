use rten::Model;
use rten_tensor::Tensor;
use rten_tensor::NdTensor;
use rten_tensor::prelude::*;
use std::collections::HashMap;

struct WordPieceTokenizer {
    vocab: HashMap<String, i32>,
    unk_id: i32,
    cls_id: i32,
    sep_id: i32,
}

impl WordPieceTokenizer {
    fn new(vocab_str: &str) -> Self {
        let mut vocab = HashMap::new();
        let mut unk_id = 100;
        let mut cls_id = 101;
        let mut sep_id = 102;
        
        for (idx, line) in vocab_str.lines().enumerate() {
            let token = line.trim();
            vocab.insert(token.to_string(), idx as i32);
            if token == "[UNK]" {
                unk_id = idx as i32;
            } else if token == "[CLS]" {
                cls_id = idx as i32;
            } else if token == "[SEP]" {
                sep_id = idx as i32;
            }
        }
        
        WordPieceTokenizer { vocab, unk_id, cls_id, sep_id }
    }

    fn tokenize(&self, text: &str) -> Vec<i32> {
        let mut input_ids = vec![self.cls_id];
        let clean_text = text.to_lowercase();
        
        for word in clean_text.split_whitespace() {
            let chars: Vec<char> = word.chars().collect();
            let mut start = 0;
            while start < chars.len() {
                let mut end = chars.len();
                let mut cur_subtoken = None;
                while start < end {
                    let mut substr: String = chars[start..end].iter().collect();
                    if start > 0 {
                        substr = format!("##{}", substr);
                    }
                    if let Some(&id) = self.vocab.get(&substr) {
                        cur_subtoken = Some(id);
                        break;
                    }
                    end -= 1;
                }
                if let Some(id) = cur_subtoken {
                    input_ids.push(id);
                    start = end;
                } else {
                    input_ids.push(self.unk_id);
                    break;
                }
            }
        }
        
        input_ids.push(self.sep_id);
        input_ids
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = Model::load_file(".brainpipe_cache/all-MiniLM-L6-v2.onnx")?;
    let vocab_str = std::fs::read_to_string(".brainpipe_cache/vocab.txt")?;
    let tokenizer = WordPieceTokenizer::new(&vocab_str);
    
    let sentence = "Hello, world! This is a semantic chunking test.";
    let input_ids = tokenizer.tokenize(sentence);
    let ids_len = input_ids.len();
    
    println!("Tokens: {:?}", input_ids);
    
    let input_ids_tensor = Tensor::from_data(&[1, ids_len], input_ids);
    let attention_mask_tensor = Tensor::from_data(&[1, ids_len], vec![1i32; ids_len]);
    let token_type_ids_tensor = Tensor::from_data(&[1, ids_len], vec![0i32; ids_len]);
    
    let inputs = vec![
        (model.node_id("input_ids")?, input_ids_tensor.view().into()),
        (model.node_id("attention_mask")?, attention_mask_tensor.view().into()),
        (model.node_id("token_type_ids")?, token_type_ids_tensor.view().into()),
    ];
    
    let output_names = vec![model.node_id("last_hidden_state")?];
    let outputs = model.run(inputs, &output_names, None)?;
    
    let output_val = &outputs[0];
    let tensor: NdTensor<f32, 3> = output_val.clone().try_into()?;
    
    println!("Output shape: {:?}", tensor.shape());
    
    // Mean pooling
    let mut sum_vec = vec![0.0f32; 384];
    for seq_idx in 0..ids_len {
        for dim_idx in 0..384 {
            sum_vec[dim_idx] += tensor[[0, seq_idx, dim_idx]];
        }
    }
    for dim_idx in 0..384 {
        sum_vec[dim_idx] /= ids_len as f32;
    }
    
    println!("Mean pooled embedding: prefix={:?}", &sum_vec[..5]);
    
    Ok(())
}
