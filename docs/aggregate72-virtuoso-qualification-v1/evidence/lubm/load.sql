log_enable(2,1);
DB.DBA.ld_dir('/import', 'data.nt', 'http://bench/aggregate72/lubm');
DB.DBA.rdf_loader_run();
checkpoint;
select ll_file,ll_state,ll_error from DB.DBA.load_list;
