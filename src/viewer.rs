use crate::{query::{self,Chunk,Detail,Filter,Summary},storage::recovery::viewer_catalog};
use anyhow::Result;
use eframe::egui::{self,Color32,RichText};
use std::{path::PathBuf,sync::mpsc::{self,Receiver},thread,time::{Duration,Instant}};

pub fn open(root:PathBuf)->Result<()>{
    let options=eframe::NativeOptions{viewport:egui::ViewportBuilder::default()
        .with_inner_size([1060.0,720.0]).with_min_inner_size([720.0,480.0]),..Default::default()};
    eframe::run_native("Agent Watcher",options,Box::new(move|cc|{
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Ok(Box::new(Viewer::new(root)))
    })).map_err(|e|anyhow::anyhow!("{e}"))
}

enum Page{Files,Commands,Details,Sources}
enum Loaded{Files(Vec<Chunk>,String),Commands(Vec<Summary>),Details(Detail),Sources(Vec<(String,String,String,i64)>)}

struct Viewer {
    root:PathBuf,page:Page,files:Vec<Chunk>,rows:Vec<Summary>,selected:Option<Chunk>,
    item:Option<Summary>,details:Option<Detail>,part:usize,filter:Filter,agent:String,
    cursors:Vec<i64>,pending:Option<Receiver<Result<Loaded,String>>>,message:String,
    health:String,last_refresh:Instant,
    sources:Vec<(String,String,String,i64)>,exit_filter:String,
}

impl Viewer{
    fn new(root:PathBuf)->Self{
        let mut app=Self{root,page:Page::Files,files:Vec::new(),rows:Vec::new(),selected:None,item:None,
            details:None,part:0,filter:Filter::default(),agent:String::new(),cursors:vec![i64::MAX],
            pending:None,message:String::new(),health:String::new(),last_refresh:Instant::now(),sources:Vec::new(),exit_filter:String::new()};
        app.load_files();app
    }

    fn load(&mut self,f:impl FnOnce(&rusqlite::Connection)->Result<Loaded>+Send+'static){
        let (send,recv)=mpsc::channel();let root=self.root.clone();
        self.message.clear();self.pending=Some(recv);
        thread::spawn(move||{
            let result=viewer_catalog(&root).and_then(|db|f(&db)).map_err(|e|format!("{e:#}"));
            let _=send.send(result);
        });
    }
    fn load_files(&mut self){
        self.page=Page::Files;self.last_refresh=Instant::now();
        self.load(|db|Ok(Loaded::Files(query::files(db)?,query::health(db)?)));
    }
    fn load_rows(&mut self,reset:bool){
        if reset{self.cursors=vec![i64::MAX];}
        self.page=Page::Commands;self.rows.clear();
        self.filter.exit_code=if self.exit_filter.trim().is_empty(){None}else{
            match self.exit_filter.trim().parse(){Ok(value)=>Some(value),Err(_)=>{self.pending=None;self.message="Exit code must be an integer.".into();return;}}
        };
        self.filter.agent=self.agent.split(',').map(str::trim).filter(|s|!s.is_empty()).map(str::to_owned).collect();
        let filter=self.filter.clone();let file=self.selected.as_ref().map(|f|f.id);
        let before=*self.cursors.last().expect("first page cursor");
        self.load(move|db|Ok(Loaded::Commands(query::search(db,file,&filter,before,51)?)));
    }
    fn load_detail(&mut self){
        let Some(row)=&self.item else{return};
        let cid=row.id.clone();let page=self.part;
        self.page=Page::Details;self.details=None;
        self.load(move|db|Ok(Loaded::Details(query::detail(db,&cid,page)?)));
    }
    fn poll(&mut self){
        let Some(recv)=self.pending.as_ref() else{return};
        let result=match recv.try_recv(){
            Ok(r)=>r,Err(mpsc::TryRecvError::Empty)=>return,
            Err(mpsc::TryRecvError::Disconnected)=>Err("Reader stopped. Refresh to retry.".into())
        };
        self.pending=None;
        match result{
            Ok(Loaded::Files(list,status))=>{self.files=list;self.health=status;}
            Ok(Loaded::Commands(rows))=>self.rows=rows,
            Ok(Loaded::Details(detail))=>self.details=Some(detail),
            Ok(Loaded::Sources(sources))=>self.sources=sources,
            Err(msg)=>self.message=msg
        }
    }

    fn files_panel(&mut self,ui:&mut egui::Ui){
        ui.horizontal(|ui|{
            ui.heading("Log files");
            if ui.button("Refresh").clicked(){self.load_files();}
            if ui.button("Search all files").clicked(){self.selected=None;self.load_rows(true);}
        });
        ui.label(&self.health);ui.separator();
        let mut chosen=None;
        egui::ScrollArea::vertical().show(ui,|ui|{
            egui::Grid::new("files").striped(true).spacing([24.0,10.0]).show(ui,|ui|{
                ui.strong("File");ui.strong("Size");ui.strong("State");ui.end_row();
                for f in &self.files {
                    if ui.link(RichText::new(&f.name).monospace()).clicked(){chosen=Some(f.clone());}
                    ui.label(format!("{} bytes",f.bytes));ui.label(&f.state);ui.end_row();
                }
            });
            if self.files.is_empty() && self.pending.is_none() && self.message.is_empty(){ui.label("Waiting for the first captured command.");}
        });
        if let Some(file)=chosen{self.selected=Some(file);self.load_rows(true);}
    }

    fn commands_panel(&mut self,ui:&mut egui::Ui){
        ui.heading(self.selected.as_ref().map_or("All files",|f|f.name.as_str()));
        egui::Grid::new("filters").num_columns(4).spacing([8.0,5.0]).show(ui,|ui|{
            for (a,x,b,y) in [
                ("Agent(s)",&mut self.agent,"Command",&mut self.filter.command),
                ("Output",&mut self.filter.output,"Capture status",&mut self.filter.capture),
                ("From (RFC 3339)",&mut self.filter.from,"Until (RFC 3339)",&mut self.filter.until)
            ]{ui.label(a);ui.text_edit_singleline(x);ui.label(b);ui.text_edit_singleline(y);ui.end_row();}
            ui.label("Outcome");ui.text_edit_singleline(&mut self.filter.outcome);
            ui.label("Exit code");ui.text_edit_singleline(&mut self.exit_filter);ui.end_row();
        });
        ui.horizontal(|ui|{
            if ui.button("Apply filters").clicked(){self.load_rows(true);}
            if ui.button("Clear").clicked(){self.filter=Filter::default();self.agent.clear();self.exit_filter.clear();self.load_rows(true);}
            if ui.button("Refresh").clicked(){self.load_rows(false);}
            if ui.add_enabled(self.cursors.len()>1,egui::Button::new("Previous")).clicked(){self.cursors.pop();self.load_rows(false);}
            if ui.add_enabled(self.rows.len()>50,egui::Button::new("Next")).clicked(){self.cursors.push(self.rows[49].cursor);self.load_rows(false);}
        });
        ui.separator();
        let mut chosen=None;
        egui::ScrollArea::vertical().show(ui,|ui|{
            for row in self.rows.iter().take(50){
                ui.horizontal_wrapped(|ui|{ui.strong(&row.app);ui.weak(&row.time);ui.label(format!("{} · {}",row.outcome,row.capture));});
                let preview:String=query::visible(&row.command).chars().take(180).collect();
                if ui.link(if preview.is_empty(){"Open command"}else{&preview}).clicked(){chosen=Some(row.clone());}
                ui.separator();
            }
            if self.rows.is_empty() && self.pending.is_none() && self.message.is_empty(){ui.label("No matching commands.");}
        });
        if let Some(row)=chosen{self.item=Some(row);self.part=0;self.load_detail();}
    }
    fn details_panel(&mut self,ui:&mut egui::Ui){
        let Some(item)=&self.item else{return};
        ui.heading(&item.app);ui.label(format!("{}  ·  {}  ·  {} attribution",item.time,item.source,item.attribution));
        ui.label(format!("{}  ·  exit {}  ·  output {}",item.outcome,item.exit_code.map_or("unknown".into(),|c|c.to_string()),item.capture));
        ui.monospace(query::visible(&item.command));ui.separator();
        let more=self.details.as_ref().is_some_and(|d|d.has_more);
        if let Some(detail)=&self.details{ui.label(detail.files.join("  ·  "));}
        ui.horizontal(|ui|{
            if ui.add_enabled(self.part>0,egui::Button::new("Previous output")).clicked(){self.part-=1;self.load_detail();}
            if ui.add_enabled(more,egui::Button::new("Next output")).clicked(){self.part+=1;self.load_detail();}
        });
        egui::ScrollArea::both().show(ui,|ui|{
            if let Some(detail)=&self.details{
                if detail.lines.is_empty(){ui.label("No output records.");}
                for line in &detail.lines{ui.monospace(line);}
            }
        });
    }
}
impl eframe::App for Viewer{
    fn update(&mut self,ctx:&egui::Context,_:&mut eframe::Frame){
        self.poll();
        if matches!(self.page,Page::Files) && self.pending.is_none() && self.last_refresh.elapsed()>Duration::from_secs(5){self.load_files();}
        egui::TopBottomPanel::top("nav").show(ctx,|ui|{
            ui.horizontal(|ui|{
                ui.heading("Agent Watcher");ui.separator();
                if ui.button("Files").clicked(){self.load_files();}
                if ui.button("Sources").clicked(){
                    self.page=Page::Sources;self.load(|db|Ok(Loaded::Sources(query::sources(db)?)));
                }
                if matches!(self.page,Page::Details) && ui.button("Back to commands").clicked(){self.load_rows(false);}
                if self.pending.is_some(){ui.spinner();}
            });
        });
        egui::CentralPanel::default().show(ctx,|ui|{
            if !self.message.is_empty(){ui.colored_label(Color32::LIGHT_RED,&self.message);}
            match self.page{
                Page::Files=>self.files_panel(ui),Page::Commands=>self.commands_panel(ui),Page::Details=>self.details_panel(ui),
                Page::Sources=>{
                    ui.heading("Observed agents");
                    if self.sources.is_empty(){ui.label("No agent has reported a command yet.");}
                    for (agent,source,last,count) in &self.sources{
                        ui.separator();ui.strong(agent);
                        let coverage=match source.as_str(){"executor"=>"redirected stdout and stderr","windows-audit"=>"process metadata only",_=>"agent-reported tool result"};
                        ui.label(format!("{source} · {coverage}"));ui.label(format!("{count} commands · last observed {last}"));
                    }
                }
            }
        });
        ctx.request_repaint_after(Duration::from_millis(if self.pending.is_some(){100}else{1000}));
    }
}
