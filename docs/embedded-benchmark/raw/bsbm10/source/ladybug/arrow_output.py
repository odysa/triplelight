"""Build SPARQL JSON from native bulk Arrow results without per-cell JSON parsing."""
import json


def encode(table, rec):
    import pyarrow as pa
    import pyarrow.compute as pc
    n=table.num_rows
    row=pc.fill_null(pa.nulls(n,type=pa.string()),'')
    for i,var in enumerate(rec['vars']):
        col=table.column(i).combine_chunks().cast(pa.string())
        if var in rec.get('predicate_columns',[]):
            labels=list(rec['predicate_labels'])
            values=pa.array([json.dumps({'type':'uri','value':rec['predicate_labels'][label]},separators=(',',':')) for label in labels])
            indices=pc.index_in(col,value_set=pa.array(labels))
            if pc.any(pc.and_(pc.invert(pc.is_null(col)),pc.is_null(indices))).as_py():
                raise ValueError('Native relationship label missing from RDF mapping')
            col=pc.take(values,indices)
        field=pc.binary_join_element_wise(json.dumps(var)+':',col,'')
        combined=pc.if_else(pc.equal(row,''),field,pc.binary_join_element_wise(row,',',field,''))
        row=pc.if_else(pc.is_null(col),row,combined)
    row=pc.binary_join_element_wise('{',row,'}','')
    one_list=pa.ListArray.from_arrays(pa.array([0,n],type=pa.int32()),row)
    joined=pc.binary_join(one_list,',')[0].as_py()
    return ('{"head":'+json.dumps({'vars':rec['vars']})+',"results":{"bindings":['+joined+']}}').encode()
