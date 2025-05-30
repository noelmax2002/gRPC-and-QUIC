import os
import re
import matplotlib.pyplot as plt
import numpy as np

def extract_data(file_name):
    y_vals = []
    with open(file_name, 'r') as file:
        for line in file:
            match = re.search(r"Total time elapsed:\s*(\d+\.\d+)", line)
            if match:
                value = float(match.group(1))
                y_vals.append(value)  
    return y_vals

def collect_folder_data(folder_path):
    all_y = []
    for file_name in os.listdir(folder_path):
        full_path = os.path.join(folder_path, file_name)
        if os.path.isfile(full_path):
            y_vals = extract_data(full_path)
            all_y.extend(y_vals)
    return all_y

folder1 = 'tcp_only_cut' 
folder2 = 'tcp_improved_cut' 
folder3 = 'quic_only_cut'  
folder4 = 'mpquic_backup_improved_cut'
folder5 = 'mpquic_duplicate_cut'

data1 = collect_folder_data(folder1)  
data2 = collect_folder_data(folder2)  
data3 = collect_folder_data(folder3)  
data4 = collect_folder_data(folder4)  
data5 = collect_folder_data(folder5) 


plt.figure(figsize=(8, 6))

box_data = [data1, data2, data3, data4, data5]
positions = [1, 2, 3, 4, 5]
colors = ['red', 'red', 'blue', 'blue','blue']  

boxplots = plt.boxplot(box_data, positions=positions, patch_artist=True, showfliers=False)

for patch, color in zip(boxplots['boxes'], colors):
    patch.set(facecolor='white', edgecolor=color, linewidth=2)

for i, (whisker, cap, median) in enumerate(zip(
        zip(boxplots['whiskers'][::2], boxplots['whiskers'][1::2]),
        zip(boxplots['caps'][::2], boxplots['caps'][1::2]),
        boxplots['medians'])):
    color = colors[i]
    for w in whisker:
        w.set(color=color, linewidth=1.5)
    for c in cap:
        c.set(color=color, linewidth=1.5)
    median.set(color=color, linewidth=2)

plt.xticks([1,2,3,4,5], ["Tonic", "Tonic\n improved", "QUIC", "MPQUIC [A-P]\nlrtt scheduler\nimproved", "MPQUIC [A-A]\n last scheduler"])
plt.ylabel('Total time of execution [s]')
plt.yticks(np.arange(19.5,26.5,0.5))
plt.grid(True, axis='y')
plt.tight_layout()

custom_legend = [
    plt.Line2D([0], [0], color='blue', lw=2, label='QUIC'),
    plt.Line2D([0], [0], color='red', lw=2, label='TCP')
]
#plt.legend(handles=custom_legend, loc='upper right')

plt.savefig('Boxplot_cut.pdf')
plt.show()
