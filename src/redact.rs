//! Incremental masking of common credential fields. State survives byte reads.
const KEYS: &[&[u8]] = &[b"password",b"passwd",b"apikey",b"api_key",b"api-key",b"access_token",b"access-token",b"accesstoken",b"secret",b"bearer"];

#[derive(Default,Clone,Copy)]
enum State {
    #[default] Text,
    AfterKey { bearer:bool, space:bool },
    BeforeValue,
    Value { quote:Option<u8>, escaped:bool },
}

#[derive(Default)]
pub struct Redactor { state:State, prefix:Vec<u8> }

impl Redactor {
    pub fn write(&mut self,input:&[u8],out:&mut Vec<u8>){
        for &byte in input{self.byte(byte,out);}
    }
    pub fn finish(&mut self,out:&mut Vec<u8>){out.append(&mut self.prefix);self.state=State::Text;}

    fn byte(&mut self,b:u8,out:&mut Vec<u8>){
        match self.state {
            State::Text=>{
                if self.prefix.is_empty() && !matches!(b.to_ascii_lowercase(),b'a'|b'b'|b'p'|b's'){out.push(b);return;}
                self.prefix.push(b);
                while !self.prefix.is_empty(){
                    if let Some(key)=KEYS.iter().find(|key|key.len()>=self.prefix.len() && key[..self.prefix.len()].eq_ignore_ascii_case(&self.prefix)){
                        if key.len()==self.prefix.len(){
                            self.state=State::AfterKey{bearer:*key==b"bearer",space:false};out.append(&mut self.prefix);
                        }
                        break;
                    }
                    out.push(self.prefix.remove(0));
                }
            }
            State::AfterKey{bearer,space}=>{
                if b==b' ' || b==b'\t' {out.push(b);self.state=State::AfterKey{bearer,space:true};}
                else if !bearer && matches!(b,b'='|b':'){out.push(b);self.state=State::BeforeValue;}
                else if space && !b.is_ascii_whitespace(){self.begin(b,out);}
                else {self.state=State::Text;self.byte(b,out);}
            }
            State::BeforeValue=>{
                if b==b' ' || b==b'\t'{out.push(b);}
                else if b.is_ascii_whitespace(){out.push(b);self.state=State::Text;}
                else{self.begin(b,out);}
            }
            State::Value{quote:Some(q),escaped}=>{
                if b==q && !escaped{self.state=State::Text;}
                else{self.state=State::Value{quote:Some(q),escaped:b==b'\\' && !escaped};}
            }
            State::Value{quote:None,..}=>{
                if b.is_ascii_whitespace() || matches!(b,b','|b';'){self.state=State::Text;self.byte(b,out);}
            }
        }
    }
    fn begin(&mut self,b:u8,out:&mut Vec<u8>){
        out.extend_from_slice(b"[REDACTED]");
        self.state=State::Value{quote:matches!(b,b'\''|b'"').then_some(b),escaped:false};
    }
}

pub fn mask(text:&str)->String{
    let mut r=Redactor::default();let mut out=Vec::with_capacity(text.len());
    r.write(text.as_bytes(),&mut out);r.finish(&mut out);
    String::from_utf8(out).expect("UTF-8 preserved by ASCII masking")
}

pub fn argv(values:&[String])->Vec<String>{
    let mut next=false;
    values.iter().map(|s|{
        if next{next=false;return "[REDACTED]".into();}
        let key=s.trim_start_matches(['-','/']).to_ascii_lowercase();
        next=s.starts_with(['-','/']) && KEYS.contains(&key.as_bytes());
        mask(s)
    }).collect()
}
